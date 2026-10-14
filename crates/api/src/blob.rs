//! 磁盘 Blob 缓存：实现按内容寻址落盘与边下边播文件视图。

use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::task::{Context, Poll};
use std::time::Duration;

use parking_lot::{Condvar, Mutex};

use crate::error::{Error, Result};
use crate::transport::{ChunkSink, Method, Request, Transport};

/// 下载进度与完成状态。
#[derive(Debug)]
struct Progress {
    downloaded: AtomicU64,
    complete: AtomicBool,
    error: Mutex<Option<String>>,
    /// 伴随条件变量用于通知新数据写入。
    gate: Mutex<()>,
    cond: Condvar,
}

impl Progress {
    fn new(downloaded: u64, complete: bool) -> Self {
        Self {
            downloaded: AtomicU64::new(downloaded),
            complete: AtomicBool::new(complete),
            error: Mutex::new(None),
            gate: Mutex::new(()),
            cond: Condvar::new(),
        }
    }

    fn notify_finished(&self) {
        self.cond.notify_all();
    }
}

/// 「正在增长的文件」视图，实现了 `Read + Seek`。
#[derive(Debug)]
pub struct GrowingFile {
    file: std::fs::File,
    progress: Arc<Progress>,
    pos: u64,
}

impl GrowingFile {
    /// 当前已落盘字节数。
    #[must_use]
    pub fn downloaded(&self) -> u64 {
        self.progress.downloaded.load(Ordering::Acquire)
    }

    /// 下载是否已结束（成功或失败）。
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.progress.complete.load(Ordering::Acquire)
    }

    /// 等待读到至少 `wanted` 字节，或下载结束。
    fn wait_for(&self, wanted: u64) {
        while self.downloaded() < wanted && !self.is_complete() {
            let mut gate = self.progress.gate.lock();
            // 超时等待防止丢唤醒导致死锁。
            self.progress.cond.wait_for(&mut gate, Duration::from_millis(100));
        }
    }
}

impl std::io::Read for GrowingFile {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        loop {
            let available = self.downloaded();
            if self.pos < available {
                let want = buf
                    .len()
                    .min(usize::try_from(available - self.pos).unwrap_or(usize::MAX));
                // 显式全限定调用寻址与读取方法。
                std::io::Seek::seek(&mut self.file, std::io::SeekFrom::Start(self.pos))?;
                let read = std::io::Read::read(&mut self.file, &mut buf[..want])?;
                self.pos += read as u64;
                return Ok(read);
            }
            if let Some(message) = self.progress.error.lock().clone() {
                return Err(std::io::Error::other(message));
            }
            if self.is_complete() {
                return Ok(0);
            }
            self.wait_for(self.pos + 1);
        }
    }
}

impl std::io::Seek for GrowingFile {
    fn seek(&mut self, pos: std::io::SeekFrom) -> std::io::Result<u64> {
        let target = match pos {
            std::io::SeekFrom::Start(offset) => offset,
            std::io::SeekFrom::End(offset) => {
                // 寻址至文件末尾需等待下载完全结束。
                while !self.is_complete() {
                    self.wait_for(self.downloaded() + 1);
                }
                if let Some(message) = self.progress.error.lock().clone() {
                    return Err(std::io::Error::other(message));
                }
                let len = self.downloaded();
                u64::try_from(i128::from(len) + i128::from(offset))
                    .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidInput, "seek before start"))?
            }
            std::io::SeekFrom::Current(offset) => u64::try_from(i128::from(self.pos) + i128::from(offset))
                .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidInput, "seek before start"))?,
        };
        self.pos = target;
        Ok(target)
    }
}

/// 异步下载任务，由调用方驱动完成数据拉取。
pub struct Download {
    inner: Pin<Box<dyn Future<Output = ()> + Send>>,
}

impl std::fmt::Debug for Download {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Download").finish_non_exhaustive()
    }
}

impl Future for Download {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        self.inner.as_mut().poll(cx)
    }
}

/// 磁盘 blob 缓存。
#[derive(Clone)]
pub struct BlobCache {
    root: PathBuf,
    max_bytes: u64,
    transport: Arc<dyn Transport>,
}

impl std::fmt::Debug for BlobCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BlobCache")
            .field("root", &self.root)
            .field("max_bytes", &self.max_bytes)
            .finish_non_exhaustive()
    }
}

impl BlobCache {
    /// 构造（不创建目录，首次写入时再建）。
    #[must_use]
    pub fn new(root: impl Into<PathBuf>, max_bytes: u64, transport: Arc<dyn Transport>) -> Self {
        Self {
            root: root.into(),
            max_bytes,
            transport,
        }
    }

    /// 缓存根目录。
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// 容量上限。
    #[must_use]
    pub const fn max_bytes(&self) -> u64 {
        self.max_bytes
    }

    /// 正式文件路径：`{root}/{key前两位}/{key}.{ext}`。
    #[must_use]
    pub fn path_for(&self, key: &str, ext: &str) -> PathBuf {
        let bucket = key.get(0..2).unwrap_or("00");
        self.root.join(bucket).join(format!("{key}.{ext}"))
    }

    /// 下载中的临时文件路径。
    #[must_use]
    pub fn part_path(&self, key: &str, ext: &str) -> PathBuf {
        self.root.join(format!("{key}.{ext}.part"))
    }

    /// 是否已完整缓存。
    #[must_use]
    pub fn is_cached(&self, key: &str, ext: &str) -> bool {
        self.path_for(key, ext).is_file()
    }

    /// 缓存占用的总字节数。
    #[must_use]
    pub fn total_bytes(&self) -> u64 {
        walk_files(&self.root)
            .into_iter()
            .filter_map(|path| std::fs::metadata(&path).ok())
            .map(|meta| meta.len())
            .sum()
    }

    /// 按修改时间从旧到新淘汰，直到总量不超过 `target_bytes`。返回删除的字节数。
    pub fn evict_lru(&self, target_bytes: u64) -> u64 {
        let mut files: Vec<(std::time::SystemTime, u64, PathBuf)> = walk_files(&self.root)
            .into_iter()
            .filter_map(|path| {
                let meta = std::fs::metadata(&path).ok()?;
                Some((meta.modified().ok()?, meta.len(), path))
            })
            .collect();
        // 时间戳并列时以文件路径次级排序保证确定性。
        files.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.2.cmp(&b.2)));

        let mut total: u64 = files.iter().map(|(_, len, _)| *len).sum();
        let mut freed = 0;
        for (_, len, path) in files {
            if total <= target_bytes {
                break;
            }
            if std::fs::remove_file(&path).is_ok() {
                total = total.saturating_sub(len);
                freed += len;
            }
        }
        freed
    }

    /// 打开边下边播文件视图与下载任务。
    pub fn open_stream(&self, url: &str, key: &str, ext: &str) -> Result<(GrowingFile, Download)> {
        let final_path = self.path_for(key, ext);
        if final_path.is_file() {
            let file = std::fs::File::open(&final_path)?;
            let len = file.metadata()?.len();
            let progress = Arc::new(Progress::new(len, true));
            return Ok((
                GrowingFile { file, progress, pos: 0 },
                Download {
                    inner: Box::pin(async {}),
                },
            ));
        }

        std::fs::create_dir_all(&self.root)?;
        if let Some(parent) = final_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let part_path = self.part_path(key, ext);
        // 每次重新截断下载，避免 CDN 忽略 Range 导致的数据重复错位。
        std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&part_path)?;
        let progress = Arc::new(Progress::new(0, false));
        let file = std::fs::File::open(&part_path)?;
        let view = GrowingFile {
            file,
            progress: progress.clone(),
            pos: 0,
        };

        let transport = self.transport.clone();
        let url = url.to_owned();
        let task = Download {
            inner: Box::pin(async move {
                let outcome = stream_to_file(transport, &url, &part_path, &progress).await;
                match outcome {
                    Ok(()) => {
                        if let Err(error) = std::fs::rename(&part_path, &final_path) {
                            *progress.error.lock() = Some(format!("rename 失败: {error}"));
                        }
                    }
                    Err(error) => *progress.error.lock() = Some(error.to_string()),
                }
                progress.complete.store(true, Ordering::Release);
                progress.notify_finished();
            }),
        };
        Ok((view, task))
    }

    /// 下载完整文件（命中即返回），用于预热与整首下载。
    pub async fn ensure(&self, url: &str, key: &str, ext: &str) -> Result<PathBuf> {
        let (view, download) = self.open_stream(url, key, ext)?;
        download.await;
        if let Some(message) = view.progress.error.lock().clone() {
            return Err(Error::Transport(message));
        }
        Ok(self.path_for(key, ext))
    }
}

/// 收集目录下的全部文件（忽略失败）。
fn walk_files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                out.push(path);
            }
        }
    }
    out
}

/// 把 HTTP 响应流式写入 `part_path`。
async fn stream_to_file(
    transport: Arc<dyn Transport>,
    url: &str,
    part_path: &Path,
    progress: &Arc<Progress>,
) -> Result<()> {
    let mut request = Request::new(Method::Get, url.to_owned());
    // 大文件下载不设单次请求超时。
    request.timeout = None;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(part_path)?;

    let mut sink = FileSink {
        file: &mut file,
        written: 0,
        progress: progress.clone(),
        failure: None,
    };
    transport.execute_streaming(request, &mut sink).await?;
    if let Some(message) = sink.failure {
        return Err(Error::Io(message));
    }
    // 强制同步落盘，保证读取端进度与文件长度一致。
    file.sync_data()?;
    Ok(())
}

/// 把分块写盘并推进进度。
struct FileSink<'a> {
    file: &'a mut std::fs::File,
    written: u64,
    progress: Arc<Progress>,
    failure: Option<String>,
}

impl ChunkSink for FileSink<'_> {
    fn chunk(&mut self, bytes: &[u8]) {
        use std::io::Write as _;
        if self.failure.is_some() {
            return;
        }
        match self.file.write_all(bytes) {
            Ok(()) => {
                self.written += bytes.len() as u64;
                self.progress.downloaded.store(self.written, Ordering::Release);
                self.progress.notify_finished();
            }
            Err(error) => {
                self.failure = Some(error.to_string());
                self.progress.error.lock().replace(error.to_string());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::io::{Read as _, Seek as _, Write as _};

    use super::*;

    /// 生成带自增序号的独立测试临时目录。
    fn temp_root() -> PathBuf {
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let dir = std::env::temp_dir().join(format!("lyra-blob-{}-{nanos}-{seq}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn transport_with(body: Vec<u8>) -> Arc<crate::transport::MockTransport> {
        let mock = crate::transport::MockTransport::new();
        mock.on("/audio", crate::transport::Response::new(200, body));
        Arc::new(mock)
    }

    #[test]
    fn paths_are_content_addressed_and_bucketed() {
        let cache = BlobCache::new(temp_root(), 1024, transport_with(vec![]));
        let path = cache.path_for("abcdef1234", "flac");
        let normalised = path.to_string_lossy().replace('\\', "/");
        assert!(normalised.ends_with("ab/abcdef1234.flac"), "{normalised}");
        assert!(
            cache
                .part_path("abcdef1234", "flac")
                .to_string_lossy()
                .ends_with("abcdef1234.flac.part")
        );
    }

    #[tokio::test]
    async fn downloads_to_disk_and_serves_from_cache() {
        let root = temp_root();
        let body = b"fLaC-tiny-flac-payload".to_vec();
        let cache = BlobCache::new(root.clone(), 1024, transport_with(body.clone()));
        let path = cache.ensure("http://cdn/audio", "aabbccdd", "flac").await.unwrap();
        assert!(path.is_file());
        assert!(!cache.part_path("aabbccdd", "flac").exists(), "临时文件应已改名");
        let mut read = Vec::new();
        std::fs::File::open(&path).unwrap().read_to_end(&mut read).unwrap();
        assert_eq!(read, body);
        assert!(cache.is_cached("aabbccdd", "flac"));
        assert_eq!(cache.total_bytes(), body.len() as u64);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn growing_view_blocks_until_bytes_arrive_then_returns_eof() {
        let root = temp_root();
        let body = b"0123456789".to_vec();
        let cache = BlobCache::new(root.clone(), 1024, transport_with(body.clone()));
        let (mut view, download) = cache.open_stream("http://cdn/audio", "11223344", "flac").unwrap();
        download.await;

        let mut out = Vec::new();
        view.read_to_end(&mut out).unwrap();
        assert_eq!(out, body);
        // 读取完毕后返回 EOF。
        assert_eq!(view.read(&mut [0_u8; 4]).unwrap(), 0);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn seek_can_reposition_within_a_downloaded_file() {
        let root = temp_root();
        let cache = BlobCache::new(root.clone(), 1024, transport_with(b"0123456789".to_vec()));
        let (mut view, download) = cache.open_stream("http://cdn/audio", "55667788", "flac").unwrap();
        download.await;
        view.seek(std::io::SeekFrom::Start(5)).unwrap();
        let mut out = Vec::new();
        view.read_to_end(&mut out).unwrap();
        assert_eq!(out, b"56789");
        view.seek(std::io::SeekFrom::End(-2)).unwrap();
        let mut tail = [0_u8; 2];
        view.read_exact(&mut tail).unwrap();
        assert_eq!(&tail, b"89");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn cached_file_short_circuits_the_network() {
        let root = temp_root();
        let cache = BlobCache::new(root.clone(), 1024, transport_with(b"once".to_vec()));
        cache.ensure("http://cdn/audio", "deadbeef", "flac").await.unwrap();
        // 第二次请求使用失效传输验证直接命中本地缓存。
        let failing = BlobCache::new(root.clone(), 1024, Arc::new(crate::transport::MockTransport::new()));
        failing.ensure("http://cdn/audio", "deadbeef", "flac").await.unwrap();
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn evict_lru_frees_oldest_files_first() {
        let root = temp_root();
        let cache = BlobCache::new(root.clone(), 1024, transport_with(vec![]));
        for (index, name) in ["aaa", "bbb", "ccc"].iter().enumerate() {
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let path = cache.path_for(name, "flac");
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            let mut file = std::fs::File::create(&path).unwrap();
            file.write_all(&[0_u8; 100]).unwrap();
            // 显式指定修改时间避免文件系统时间戳粒度并列。
            let stamp =
                std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000 + index as u64);
            file.set_modified(stamp).unwrap();
        }
        assert_eq!(cache.total_bytes(), 300);
        let freed = cache.evict_lru(150);
        assert_eq!(freed, 200, "应删掉最旧的两个");
        assert!(cache.is_cached("ccc", "flac"), "最新的应保留");
        assert!(!cache.is_cached("aaa", "flac"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn download_failure_surfaces_as_a_read_error() {
        let root = temp_root();
        // 无桩应答模拟网络传输异常。
        let cache = BlobCache::new(root.clone(), 1024, Arc::new(crate::transport::MockTransport::new()));
        let (mut view, download) = cache.open_stream("http://cdn/missing", "99999999", "flac").unwrap();
        download.await;
        let mut out = Vec::new();
        let error = view.read_to_end(&mut out).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::Other);
        let _ = std::fs::remove_dir_all(&root);
    }
}
