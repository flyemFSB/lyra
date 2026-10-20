//! Blob 缓存与音频引擎之间的流式数据桥接

use lyra_api::blob::BlobCache;
use lyra_audio::{FetchError, FetchResult, Fetcher, ReadSeek, TrackRef};

/// 基于 BlobCache 实现的流式音频抓取器
#[derive(Debug, Clone)]
pub struct BlobFetcher {
    blobs: BlobCache,
    runtime: tokio::runtime::Handle,
}

impl BlobFetcher {
    /// 创建音频抓取器实例
    #[must_use]
    pub fn new(blobs: BlobCache, runtime: tokio::runtime::Handle) -> Self {
        Self { blobs, runtime }
    }
}

impl Fetcher for BlobFetcher {
    fn open(&self, track: &TrackRef) -> FetchResult<Box<dyn ReadSeek + Send>> {
        let ext = if track.ext.is_empty() { "mp3" } else { &track.ext };
        let (view, download) = self
            .blobs
            .open_stream(&track.url, &track.hash, ext)
            .map_err(|error| FetchError::Message(error.to_string()))?;
        // 启动后台下载任务供流式读取
        let handle = self.runtime.spawn(download);
        // 保留 JoinHandle：任务异常结束时至少留下一条日志（panic=abort 时进程直接退出）
        self.runtime.spawn(async move {
            if let Err(error) = handle.await {
                tracing::warn!(%error, "音频下载任务异常结束");
            }
        });
        Ok(Box::new(view))
    }
}
