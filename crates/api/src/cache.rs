//! 内存响应缓存：基于 TinyLFU 容量淘汰与分级 TTL 策略。

use std::sync::Arc;
use std::time::{Duration, Instant};

use moka::sync::Cache;

/// 缓存新鲜度策略
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Freshness {
    /// 写操作或副作用请求（不缓存并追加时间戳扰动）
    SideEffect,
    /// 指定存活时长的缓存策略
    Ttl(Duration),
    /// 不缓存且不追加时间戳
    NoStore,
}

impl Freshness {
    /// 推荐流（5 分钟）
    pub const FLOW: Self = Self::Ttl(Duration::from_secs(300));
    /// 歌单详情（30 秒）
    pub const PLAYLIST: Self = Self::Ttl(Duration::from_secs(30));
    /// 音频播放地址（60 秒）
    pub const SONG_URL: Self = Self::Ttl(Duration::from_secs(60));
    /// 目录与电台分类（10 分钟）
    pub const CATALOG: Self = Self::Ttl(Duration::from_secs(600));
    /// 账号与 VIP 状态（30 秒）
    pub const ACCOUNT: Self = Self::Ttl(Duration::from_secs(30));

    /// 获取当前策略的有效 TTL
    #[must_use]
    pub const fn ttl(self) -> Option<Duration> {
        match self {
            Self::Ttl(d) => Some(d),
            Self::SideEffect | Self::NoStore => None,
        }
    }
}

#[derive(Debug, Clone)]
struct Entry {
    expires_at: Instant,
    body: Arc<Vec<u8>>,
}

/// 内存响应缓存
#[derive(Debug, Clone)]
pub struct TtlCache {
    inner: Cache<String, Arc<Entry>>,
}

impl TtlCache {
    /// 创建指定容量上限的缓存实例
    #[must_use]
    pub fn new(max_capacity: u64) -> Self {
        Self {
            inner: Cache::builder().max_capacity(max_capacity).build(),
        }
    }

    /// 获取未过期的缓存数据
    #[must_use]
    pub fn get(&self, key: &str) -> Option<Arc<Vec<u8>>> {
        let entry = self.inner.get(key)?;
        if entry.expires_at <= Instant::now() {
            self.inner.invalidate(key);
            return None;
        }
        Some(entry.body.clone())
    }

    /// 写入缓存条目
    pub fn insert(&self, key: impl Into<String>, body: Arc<Vec<u8>>, ttl: Duration) {
        self.inner.insert(
            key.into(),
            Arc::new(Entry {
                expires_at: Instant::now() + ttl,
                body,
            }),
        );
    }

    /// 按键前缀批量失效缓存
    pub fn invalidate_prefix(&self, prefix: &str) {
        // 先过滤再克隆：命中的键通常远少于全部键
        let keys: Vec<String> = self
            .inner
            .iter()
            .filter(|(key, _)| key.starts_with(prefix))
            .map(|(key, _)| key.to_string())
            .collect();
        for key in keys {
            self.inner.invalidate(&key);
        }
    }

    /// 清空所有缓存条目
    pub fn clear(&self) {
        self.inner.invalidate_all();
        self.inner.run_pending_tasks();
    }

    /// 获取当前有效条目数
    pub fn len(&self) -> u64 {
        self.inner.run_pending_tasks();
        self.inner.entry_count()
    }

    /// 检查缓存是否为空
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn body(text: &str) -> Arc<Vec<u8>> {
        Arc::new(text.as_bytes().to_vec())
    }

    #[test]
    fn round_trips_within_ttl() {
        let cache = TtlCache::new(64);
        cache.insert("a", body("1"), Duration::from_secs(60));
        assert_eq!(cache.get("a").unwrap().as_slice(), b"1");
        assert!(cache.get("missing").is_none());
    }

    #[test]
    fn expired_entry_is_a_miss() {
        let cache = TtlCache::new(64);
        cache.insert("a", body("1"), Duration::ZERO);
        assert!(cache.get("a").is_none());
    }

    #[test]
    fn invalidate_prefix_only_touches_that_namespace() {
        let cache = TtlCache::new(64);
        cache.insert("playlist:1", body("1"), Duration::from_secs(60));
        cache.insert("playlist:2", body("2"), Duration::from_secs(60));
        cache.insert("flow:daily", body("3"), Duration::from_secs(60));
        cache.invalidate_prefix("playlist:");
        assert!(cache.get("playlist:1").is_none());
        assert!(cache.get("playlist:2").is_none());
        assert!(cache.get("flow:daily").is_some());
    }

    #[test]
    fn freshness_ttl_table() {
        assert_eq!(Freshness::SideEffect.ttl(), None);
        assert_eq!(Freshness::NoStore.ttl(), None);
        assert_eq!(Freshness::PLAYLIST.ttl(), Some(Duration::from_secs(30)));
        assert_eq!(Freshness::SONG_URL.ttl(), Some(Duration::from_secs(60)));
    }

    #[test]
    fn clear_empties_the_cache() {
        let cache = TtlCache::new(64);
        cache.insert("a", body("1"), Duration::from_secs(60));
        assert_eq!(cache.len(), 1);
        cache.clear();
        assert!(cache.is_empty());
    }
}
