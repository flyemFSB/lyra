//! 磁盘 Blob 缓存：实现按内容寻址落盘与边下边播文件视图。

use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use dashmap::DashMap;
use dashmap::mapref::entry::Entry as MapEntry;
use parking_lot::{Condvar, Mutex};

use crate::error::{Error, Result};
use crate::transport::{ChunkSink, Method, Request, Transport};

/// 数据流停滞超时阈值（30 秒未收到数据判定为停滞）
const STALL_TIMEOUT: Duration = Duration::from_secs(30);

/// 下载进度与完成状态
#[derive(Debug)]
struct Progress {
    downloaded: AtomicU64,
    complete: AtomicBool,
    error: Mutex<Option<String>>,
    /// 最近接收数据的时间戳
    last_byte_at: Mutex<Instant>,
    /// 数据写入通知锁与条件变量
    gate: Mutex<()>,
    cond: Condvar,
}

impl Progress {
    fn new(downloaded: u64, complete: bool) -> Self {
        Self {
            downloaded: AtomicU64::new(downloaded),
            complete: AtomicBool::new(complete),
            error: Mutex::new(None),
            last_byte_at: Mutex::new(Instant::now()),
            gate: Mutex::new(()),
            cond: Condvar::new(),
        }
    }

    fn notify_finished(&self) {
        self.cond.notify_all();
    }

    /// 刷新最后活跃时间
    fn touch(&self) {
        *self.last_byte_at.lock() = Instant::now();
    }

    /// 检查下载是否长时间停滞
    fn stalled(&self) -> bool {
        self.last_byte_at.lock().elapsed() >= STALL_TIMEOUT
    }
}

/// 在途下载收尾守卫
struct InflightGuard {
    cache: BlobCache,
    flight_key: String,
    progress: Arc<Progress>,
    finished: bool,
}

impl Drop for InflightGuard {
    fn drop(&mut self) {
        // 先注销在途登记再标记完成，避免同名文件竞争
        self.cache.inflight.remove(&self.flight_key);
        if !self.finished {
            self.progress
                .error
                .lock()
                .get_or_insert_with(|| "下载被取消".to_owned());
        }
        self.progress.complete.store(true, Ordering::Release);
        self.progress.notify_finished();
    }
}

/// 边下边播文件流（实现 Read 与 Seek）
#[derive(Debug)]
pub struct GrowingFile {
    file: std::fs::File,
    progress: Arc<Progress>,
    pos: u64,
}

impl GrowingFile {
    /// 当前已落盘字节数
    #[must_use]
    pub fn downloaded(&self) -> u64 {
        self.progress.downloaded.load(Ordering::Acquire)
    }

    /// 下载是否已结束（成功或失败）
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.progress.complete.load(Ordering::Acquire)
    }

    /// 等待数据流到达指定字节数
    fn wait_for(&self, wanted: u64) -> std::io::Result<()> {
        while self.downloaded() < wanted && !self.is_complete() {
            let mut gate = self.progress.gate.lock();
            // 轮询并等待数据写入通知
            self.progress.cond.wait_for(&mut gate, Duration::from_millis(100));
            drop(gate);
            if self.progress.stalled() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    format!(
                        "下载停滞：{} 秒内未收到新数据（已收到 {} 字节）",
                        STALL_TIMEOUT.as_secs(),
                        self.downloaded()
                    ),
                ));
            }
        }
        Ok(())
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
            self.wait_for(self.pos + 1)?;
        }
    }
}

impl std::io::Seek for GrowingFile {
    fn seek(&mut self, pos: std::io::SeekFrom) -> std::io::Result<u64> {
        let target = match pos {
            std::io::SeekFrom::Start(offset) => offset,
            std::io::SeekFrom::End(offset) => {
                // 从末尾寻址需等待下载完全结束
                while !self.is_complete() {
                    self.wait_for(self.downloaded() + 1)?;
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

/// 异步下载任务句柄
pub struct Download {
    inner: Pin<Box<dyn Future<Output = ()> + Send>>,
}

impl std::fmt::Debug for Download {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Download").finish_non_exhaustive()
    }
}

impl Download {
    /// 创建空操作占位任务（缓存命中或复用在途任务时使用）
    fn idle() -> Self {
        Self {
            inner: Box::pin(async {}),
        }
    }
}

impl Future for Download {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        self.inner.as_mut().poll(cx)
    }
}

/// 磁盘 Blob 缓存管理器
#[derive(Clone)]
pub struct BlobCache {
    root: PathBuf,
    /// 缓存容量上限（字节）
    max_bytes: Arc<AtomicU64>,
    transport: Arc<dyn Transport>,
    /// 在途下载表（按 `{key}.{ext}` 去重，避免并发写入冲突）
    inflight: Arc<DashMap<String, Arc<Progress>>>,
}

impl std::fmt::Debug for BlobCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BlobCache")
            .field("root", &self.root)
            .field("max_bytes", &self.max_bytes())
            .finish_non_exhaustive()
    }
}

impl BlobCache {
    /// 创建磁盘缓存实例
    #[must_use]
    pub fn new(root: impl Into<PathBuf>, max_bytes: u64, transport: Arc<dyn Transport>) -> Self {
        Self {
            root: root.into(),
            max_bytes: Arc::new(AtomicU64::new(max_bytes)),
            transport,
            inflight: Arc::new(DashMap::new()),
        }
    }

    /// 缓存根目录
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// 获取当前容量上限
    #[must_use]
    pub fn max_bytes(&self) -> u64 {
        self.max_bytes.load(Ordering::Relaxed)
    }

    /// 设置容量上限并触发超额淘汰
    pub fn set_max_bytes(&self, bytes: u64) {
        self.max_bytes.store(bytes, Ordering::Relaxed);
        if self.total_bytes() > bytes {
            self.evict_lru(bytes);
        }
    }

    /// 获取已完成缓存文件的绝对路径
    #[must_use]
    pub fn path_for(&self, key: &str, ext: &str) -> PathBuf {
        // 分桶前两位防目录穿越
        let bucket = key
            .get(0..2)
            .filter(|bucket| *bucket != "." && *bucket != "..")
            .unwrap_or("00");
        self.root.join(bucket).join(format!("{key}.{ext}"))
    }

    /// 获取下载临时文件路径
    #[must_use]
    pub fn part_path(&self, key: &str, ext: &str) -> PathBuf {
        self.root.join(format!("{key}.{ext}.part"))
    }

    /// 检查指定资源是否已完整缓存
    #[must_use]
    pub fn is_cached(&self, key: &str, ext: &str) -> bool {
        self.path_for(key, ext).is_file()
    }

    /// 获取缓存占用总字节数
    #[must_use]
    pub fn total_bytes(&self) -> u64 {
        walk_files(&self.root)
            .into_iter()
            .filter_map(|path| std::fs::metadata(&path).ok())
            .map(|meta| meta.len())
            .sum()
    }

    /// 检查指定路径是否为当前在途写入的临时文件
    fn is_being_written(&self, path: &Path) -> bool {
        path.file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.strip_suffix(".part"))
            .is_some_and(|flight_key| self.inflight.contains_key(flight_key))
    }

    /// 按 LRU 淘汰旧缓存至目标大小，返回释放的字节数
    pub fn evict_lru(&self, target_bytes: u64) -> u64 {
        let mut files: Vec<(std::time::SystemTime, u64, PathBuf)> = walk_files(&self.root)
            .into_iter()
            .filter(|path| !self.is_being_written(path))
            .filter_map(|path| {
                let meta = std::fs::metadata(&path).ok()?;
                Some((meta.modified().ok()?, meta.len(), path))
            })
            .collect();
        // 按修改时间与路径排序保证确定性
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

    /// 校验路径片段字符安全性
    fn is_safe_path_component(value: &str) -> bool {
        !value.is_empty()
            && value.len() <= 64
            && value != "."
            && value != ".."
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
    }

    /// 打开流式读取句柄（并发请求自动复用在途任务）
    pub fn open_stream(&self, url: &str, key: &str, ext: &str) -> Result<(GrowingFile, Download)> {
        // 拒绝非法路径字符与前导点
        if !Self::is_safe_path_component(key) || key.starts_with('.') || !Self::is_safe_path_component(ext) {
            return Err(Error::Io(format!(
                "非法的缓存标识：key={key:?} ext={ext:?}（只允许字母数字与 . - _）"
            )));
        }
        let final_path = self.path_for(key, ext);
        if final_path.is_file() {
            return self.finished(key, ext);
        }

        let flight_key = format!("{key}.{ext}");
        match self.inflight.entry(flight_key.clone()) {
            // 复用在途下载的只读视图
            MapEntry::Occupied(occupied) => {
                let progress = occupied.get().clone();
                drop(occupied);
                let file = match std::fs::File::open(self.part_path(key, ext)) {
                    Ok(file) => file,
                    // 临时文件已完成重命名时直接读取正式文件
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        if let Some(message) = progress.error.lock().clone() {
                            return Err(Error::Transport(message));
                        }
                        return self.finished(key, ext);
                    }
                    Err(error) => return Err(error.into()),
                };
                Ok((GrowingFile { file, progress, pos: 0 }, Download::idle()))
            }
            MapEntry::Vacant(vacant) => {
                std::fs::create_dir_all(&self.root)?;
                if let Some(parent) = final_path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                let part_path = self.part_path(key, ext);
                // 创建或截断临时文件
                std::fs::OpenOptions::new()
                    .create(true)
                    .write(true)
                    .truncate(true)
                    .open(&part_path)?;
                let progress = Arc::new(Progress::new(0, false));
                // 注册在途状态
                let file = std::fs::File::open(&part_path)?;
                vacant.insert(progress.clone());
                let view = GrowingFile {
                    file,
                    progress: progress.clone(),
                    pos: 0,
                };

                let cache = self.clone();
                let url = url.to_owned();
                // 在途清理守卫
                let guard = InflightGuard {
                    cache: self.clone(),
                    flight_key,
                    progress: progress.clone(),
                    finished: false,
                };
                let task = Download {
                    inner: Box::pin(async move {
                        let mut guard = guard;
                        let outcome = stream_to_file(cache.transport.clone(), &url, &part_path, &progress).await;
                        match outcome {
                            Ok(()) => {
                                if let Err(error) = std::fs::rename(&part_path, &final_path) {
                                    *progress.error.lock() = Some(format!("rename 失败: {error}"));
                                    let _ = std::fs::remove_file(&part_path);
                                }
                            }
                            Err(error) => {
                                *progress.error.lock() = Some(error.to_string());
                                let _ = std::fs::remove_file(&part_path);
                            }
                        }
                        // 标记成功
                        guard.finished = true;
                        // 超限淘汰
                        let limit = cache.max_bytes();
                        if cache.total_bytes() > limit {
                            cache.evict_lru(limit);
                        }
                    }),
                };
                Ok((view, task))
            }
        }
    }

    /// 打开已完成缓存文件的只读视图
    fn finished(&self, key: &str, ext: &str) -> Result<(GrowingFile, Download)> {
        let file = std::fs::File::open(self.path_for(key, ext))?;
        let len = file.metadata()?.len();
        Ok((
            GrowingFile {
                file,
                progress: Arc::new(Progress::new(len, true)),
                pos: 0,
            },
            Download::idle(),
        ))
    }

    /// 确保完整文件已缓存落盘，支持复用在途下载
    pub async fn ensure(&self, url: &str, key: &str, ext: &str) -> Result<PathBuf> {
        let (view, download) = self.open_stream(url, key, ext)?;
        download.await;
        if let Some(message) = view.progress.error.lock().clone() {
            return Err(Error::Transport(message));
        }
        if !self.is_cached(key, ext) {
            return Err(Error::Transport("下载尚未完成".to_owned()));
        }
        Ok(self.path_for(key, ext))
    }
}

/// 递归收集目录下所有文件
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

/// 流式写入 HTTP 响应至临时文件
async fn stream_to_file(
    transport: Arc<dyn Transport>,
    url: &str,
    part_path: &Path,
    progress: &Arc<Progress>,
) -> Result<()> {
    let mut request = Request::new(Method::Get, url.to_owned());
    // 流式传输依赖读超时与停滞检测
    request.timeout = None;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(part_path)?;

    let (written, expected, failure) = {
        let mut sink = FileSink {
            file: &mut file,
            written: 0,
            expected: None,
            progress: progress.clone(),
            failure: None,
        };
        transport.execute_streaming(request, &mut sink).await?;
        (sink.written, sink.expected, sink.failure.take())
    };
    if let Some(message) = failure {
        return Err(Error::Io(message));
    }
    verify_length(written, expected)?;
    // 强制同步落盘
    file.sync_data()?;
    Ok(())
}

/// 校验下载字节数与预设长度
fn verify_length(written: u64, expected: Option<u64>) -> Result<()> {
    if written == 0 {
        return Err(Error::Io("下载不完整：上游返回 0 字节".to_owned()));
    }
    match expected {
        Some(expected) if written != expected => Err(Error::Io(format!(
            "下载不完整：收到 {written} 字节，上游声明 {expected} 字节"
        ))),
        _ => Ok(()),
    }
}

/// 文件写入接收器
struct FileSink<'a> {
    file: &'a mut std::fs::File,
    written: u64,
    expected: Option<u64>,
    progress: Arc<Progress>,
    failure: Option<String>,
}

impl ChunkSink for FileSink<'_> {
    fn expected_len(&mut self, len: Option<u64>) {
        self.expected = len;
    }

    fn chunk(&mut self, bytes: &[u8]) {
        use std::io::Write as _;
        if self.failure.is_some() {
            return;
        }
        match self.file.write_all(bytes) {
            Ok(()) => {
                self.written += bytes.len() as u64;
                self.progress.downloaded.store(self.written, Ordering::Release);
                self.progress.touch();
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
    async fn eviction_spares_in_flight_downloads_but_reclaims_orphans() {
        let root = temp_root();
        let cache = BlobCache::new(root.clone(), 1024, transport_with(vec![]));
        // 在途表登记下载并写入部分临时数据，模拟未完成的写入窗口。
        cache
            .inflight
            .insert("aabbccdd.flac".to_owned(), Arc::new(Progress::new(10, false)));
        let part = cache.part_path("aabbccdd", "flac");
        std::fs::write(&part, [b'x'; 10]).unwrap();
        assert_eq!(cache.total_bytes(), 10);

        // 淘汰不能碰正在写的文件，否则它随后的 rename 会失败。
        assert_eq!(cache.evict_lru(0), 0, "不得淘汰在途下载的 .part");
        assert!(part.is_file());

        // 登记消失（硬杀进程 / 下载已结束）后，残片就是孤儿，必须照常回收。
        cache.inflight.clear();
        assert_eq!(cache.evict_lru(0), 10, "孤儿 .part 必须被回收，否则磁盘只涨不落");
        assert!(!part.exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn dropping_a_download_releases_the_in_flight_entry() {
        let root = temp_root();
        let mock = Arc::new(crate::transport::MockTransport::new());
        mock.on("/audio", crate::transport::Response::new(200, b"0123456789".to_vec()));
        let cache = BlobCache::new(root.clone(), 1024, mock.clone());

        // 调用方拿到 Download 却没有驱动它（取消 / 直接丢掉）。
        let (_view, download) = cache.open_stream("http://cdn/audio", "eeff0011", "flac").unwrap();
        assert_eq!(cache.inflight.len(), 1, "在途登记应已建立");
        drop(download);
        assert!(
            cache.inflight.is_empty(),
            "丢掉 Download 必须摘掉在途登记，否则这个 key 会永久卡住"
        );

        // 再打开必须真的重新发起下载，而不是去读一个没人写的 .part。
        let (mut view, download) = cache.open_stream("http://cdn/audio", "eeff0011", "flac").unwrap();
        download.await;
        let mut out = Vec::new();
        view.read_to_end(&mut out).unwrap();
        assert_eq!(out, b"0123456789");
        assert_eq!(mock.count(), 1, "重新发起的下载只应请求一次");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn upstream_path_traversal_in_the_cache_key_is_rejected() {
        let root = temp_root();
        let cache = BlobCache::new(root.clone(), 1024, transport_with(vec![]));
        // key / ext 直接来自上游 JSON 的 `hash` / `extname`。
        for (key, ext) in [
            ("../../evil", "flac"),
            ("aabb", "../../../evil"),
            ("a/b", "flac"),
            ("a\\b", "flac"),
            ("", "flac"),
            (".", "flac"),
            ("..", "flac"),
        ] {
            assert!(
                cache.open_stream("http://cdn/audio", key, ext).is_err(),
                "必须拒绝 {key:?} / {ext:?}"
            );
        }
        // 正常值不受影响（含合法的点号扩展名）。
        assert!(cache.open_stream("http://cdn/audio", "aabbccdd", "flac").is_ok());
        assert!(cache.open_stream("http://cdn/audio", "aabbccdd", ".flac").is_ok());
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

    #[test]
    fn stalled_download_fails_instead_of_blocking_forever() {
        let root = temp_root();
        let cache = BlobCache::new(root.clone(), 1024, transport_with(vec![]));
        let (view, _download) = cache.open_stream("http://cdn/audio", "aabb0011", "flac").unwrap();
        // 把「最后一次收到字节」推回超时之前，模拟 CDN 只建连不发数据（不 RST）。
        let long_ago = Instant::now()
            .checked_sub(STALL_TIMEOUT + Duration::from_secs(1))
            .unwrap();
        *view.progress.last_byte_at.lock() = long_ago;
        let error = view.wait_for(1).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn concurrent_open_of_one_song_shares_a_single_download() {
        let root = temp_root();
        let mock = Arc::new(crate::transport::MockTransport::new());
        mock.on("/audio", crate::transport::Response::new(200, b"0123456789".to_vec()));
        let cache = BlobCache::new(root.clone(), 1024, mock.clone());

        let (mut first, first_download) = cache.open_stream("http://cdn/audio", "ccddee", "flac").unwrap();
        let (mut second, second_download) = cache.open_stream("http://cdn/audio", "ccddee", "flac").unwrap();
        first_download.await;
        second_download.await;

        let (mut a, mut b) = (Vec::new(), Vec::new());
        first.read_to_end(&mut a).unwrap();
        second.read_to_end(&mut b).unwrap();
        assert_eq!(a, b"0123456789");
        assert_eq!(b, b"0123456789", "两路读取必须都拿到完整文件，不能被截断");
        assert_eq!(mock.count(), 1, "同一首歌并发打开只应发一次下载请求");
        assert!(cache.is_cached("ccddee", "flac"));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 声明了长度却只给一半数据的传输（模拟门户拦截 / 上游提前断开）。
    struct TruncatingTransport;

    #[async_trait::async_trait]
    impl Transport for TruncatingTransport {
        async fn execute(&self, _request: Request) -> Result<crate::transport::Response> {
            unreachable!("只走流式路径")
        }

        async fn execute_streaming(&self, _request: Request, sink: &mut (dyn ChunkSink + Send)) -> Result<()> {
            sink.expected_len(Some(10));
            sink.chunk(b"0123");
            Ok(())
        }
    }

    #[tokio::test]
    async fn truncated_download_is_never_cached_as_a_finished_file() {
        let root = temp_root();
        let cache = BlobCache::new(root.clone(), 1024, Arc::new(TruncatingTransport));
        let error = cache.ensure("http://cdn/audio", "ffeeddcc", "flac").await.unwrap_err();
        assert!(error.to_string().contains("下载不完整"), "{error}");
        assert!(!cache.is_cached("ffeeddcc", "flac"), "半成品不得成为正式缓存");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn leading_dot_keys_cannot_escape_the_cache_root() {
        let root = temp_root();
        let cache = BlobCache::new(root.clone(), 1024, transport_with(vec![b'a'; 4]));
        // `..x` / `.hidden` 这类键会通过字符白名单，但拼出来的桶目录是 `..` 或空，必须直接拒绝。
        for key in ["..x", "...abc", ".", "..", ".hidden"] {
            assert!(
                cache.ensure("http://cdn/audio/x", key, "flac").await.is_err(),
                "以点开头的缓存键必须被拒绝：{key}"
            );
        }
        assert!(cache.ensure("http://cdn/audio/x", "aa112233", "flac").await.is_ok());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn download_evicts_when_over_the_configured_limit() {
        let root = temp_root();
        // 上限 30 字节、每首 20 字节：第二首落盘后必须淘汰最旧的一份。
        let cache = BlobCache::new(root.clone(), 30, transport_with(vec![b'a'; 20]));
        cache.ensure("http://cdn/audio/1", "aa000001", "flac").await.unwrap();
        assert_eq!(cache.total_bytes(), 20);
        cache.ensure("http://cdn/audio/2", "bb000002", "flac").await.unwrap();
        assert!(
            cache.total_bytes() <= 30,
            "落盘后必须收敛到上限内：{}",
            cache.total_bytes()
        );

        // 设置页改小上限后立即收敛，不必重启。
        cache.set_max_bytes(10);
        assert!(
            cache.total_bytes() <= 10,
            "改小上限后应立即淘汰：{}",
            cache.total_bytes()
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
