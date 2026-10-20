//! 单飞并发去重：合并同键并发请求并广播结果。

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
#[cfg(test)]
use std::{future::Future, pin::Pin};

use async_channel::Sender;
use dashmap::DashMap;
use dashmap::mapref::entry::Entry;
use parking_lot::Mutex;

use crate::error::{Error, Result};

/// 结果发布钩子（仅测试编译）
///
/// 在“结果已发布、在途登记尚未摘除”的窗口内被调用，
/// 用于验证迟到的并发调用方能拿到已发布的结果，而不会重复执行上游请求。
#[cfg(test)]
type PublishHook = Box<dyn Fn() -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync + 'static>;

#[derive(Debug)]
struct Flight<T> {
    done: Mutex<Option<Result<T>>>,
    waiters: Mutex<Vec<Sender<Result<T>>>>,
}

impl<T> Default for Flight<T> {
    fn default() -> Self {
        Self {
            done: Mutex::new(None),
            waiters: Mutex::new(Vec::new()),
        }
    }
}

/// 执行者异常退出守卫（向所有等待者广播失败）
struct LeaderGuard<'a, T: Clone + Send + Sync + 'static> {
    owner: &'a SingleFlight<T>,
    key: String,
    flight: Arc<Flight<T>>,
    finished: bool,
}

impl<T: Clone + Send + Sync + 'static> Drop for LeaderGuard<'_, T> {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        let error = Error::Transport("single-flight leader 被取消".to_owned());
        // 先发布失败结果，再摘除在途登记：
        // 反过来做的话，并发调用者可能在窗口内成为第二个 leader，重复执行上游请求。
        *self.flight.done.lock() = Some(Err(error.clone()));
        self.owner.inflight.remove(&self.key);
        let waiters = std::mem::take(&mut *self.flight.waiters.lock());
        for waiter in waiters {
            // 派发取消错误
            let _ = waiter.try_send(Err(error.clone()));
        }
    }
}

/// 并发请求合并器
pub struct SingleFlight<T> {
    inflight: DashMap<String, Arc<Flight<T>>>,
    hits: AtomicU64,
    /// 结果发布钩子（仅测试编译）
    #[cfg(test)]
    publish_hook: Mutex<Option<PublishHook>>,
}

impl<T> std::fmt::Debug for SingleFlight<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SingleFlight")
            .field("inflight", &self.inflight.len())
            .field("hits", &self.hits.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

impl<T> Default for SingleFlight<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> SingleFlight<T> {
    /// 创建请求合并器实例
    #[must_use]
    pub fn new() -> Self {
        Self {
            inflight: DashMap::new(),
            hits: AtomicU64::new(0),
            #[cfg(test)]
            publish_hook: Mutex::new(None),
        }
    }

    /// 获取被合并拦截的请求总数
    #[must_use]
    pub fn coalesced(&self) -> u64 {
        self.hits.load(Ordering::Relaxed)
    }
}

impl<T: Clone + Send + Sync + 'static> SingleFlight<T> {
    /// 执行异步任务并对相同键去重合并
    pub async fn run<F, Fut>(&self, key: impl Into<String>, work: F) -> Result<T>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<T>>,
    {
        let key = key.into();

        let (flight, leader, follower_rx) = match self.inflight.entry(key.clone()) {
            Entry::Occupied(occupied) => {
                let flight = occupied.get().clone();
                // 已完成则直接返回结果
                let done = flight.done.lock();
                if let Some(result) = done.as_ref() {
                    return result.clone();
                }
                let (tx, rx) = async_channel::bounded(1);
                flight.waiters.lock().push(tx);
                drop(done);
                self.hits.fetch_add(1, Ordering::Relaxed);
                (flight, false, Some(rx))
            }
            Entry::Vacant(vacant) => {
                let flight = Arc::new(Flight::default());
                vacant.insert(flight.clone());
                (flight, true, None)
            }
        };

        if !leader {
            return match follower_rx {
                Some(rx) => rx
                    .recv()
                    .await
                    .map_err(|_| Error::Transport("single-flight channel closed".to_owned()))?,
                // 库代码里不做 panic：无法收到结果时明确报错
                None => Err(Error::Transport("single-flight follower 缺少接收端".to_owned())),
            };
        }

        // 注册执行守卫
        let mut guard = LeaderGuard {
            owner: self,
            key: key.clone(),
            flight: flight.clone(),
            finished: false,
        };

        let result = work().await;
        guard.finished = true;
        // 先发布结果，再摘除在途登记：
        // 反过来做的话，并发调用者可能在窗口内成为第二个 leader，重复执行上游请求。
        *flight.done.lock() = Some(result.clone());
        // 仅测试：在“已发布、未摘除”的窗口内插入探测调用
        #[cfg(test)]
        self.run_publish_hook().await;
        self.inflight.remove(&key);

        let waiters = std::mem::take(&mut *flight.waiters.lock());
        for waiter in waiters {
            // 广播执行结果至所有等待者
            let _ = waiter.send(result.clone()).await;
        }
        result
    }

    /// 安装结果发布钩子（仅测试使用）
    #[cfg(test)]
    fn set_publish_hook(&self, hook: PublishHook) {
        *self.publish_hook.lock() = Some(hook);
    }

    /// 执行结果发布钩子（仅测试使用）
    #[cfg(test)]
    async fn run_publish_hook(&self) {
        let hook = self.publish_hook.lock().take();
        if let Some(hook) = hook {
            hook().await;
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Duration;

    use super::*;

    #[tokio::test]
    async fn concurrent_same_key_runs_once() {
        let flight: SingleFlight<u64> = SingleFlight::new();
        let calls = Arc::new(AtomicU64::new(0));

        let mut handles = Vec::new();
        for _ in 0..8 {
            let flight = &flight;
            let calls = calls.clone();
            handles.push(async move {
                flight
                    .run("k", || {
                        let calls = calls.clone();
                        async move {
                            calls.fetch_add(1, Ordering::SeqCst);
                            tokio::time::sleep(Duration::from_millis(20)).await;
                            Ok(42)
                        }
                    })
                    .await
                    .unwrap()
            });
        }
        let results = futures::future::join_all(handles).await;
        assert!(results.iter().all(|r| *r == 42));
        assert_eq!(calls.load(Ordering::SeqCst), 1, "同 key 并发只应真正执行一次");
        assert_eq!(flight.coalesced(), 7);
    }

    #[tokio::test]
    async fn sequential_calls_re_execute() {
        let flight: SingleFlight<u64> = SingleFlight::new();
        for expected in 1..=3 {
            let value = flight.run("k", || async move { Ok(expected) }).await.unwrap();
            assert_eq!(value, expected);
        }
        assert_eq!(flight.coalesced(), 0);
    }

    #[tokio::test]
    async fn errors_are_shared() {
        let flight: SingleFlight<u64> = SingleFlight::new();
        let err = flight.run("k", || async { Err(Error::NotLoggedIn) }).await.unwrap_err();
        assert!(matches!(err, Error::NotLoggedIn));
        // 失败后不驻留缓存，后续重新执行。
        assert_eq!(flight.run("k", || async { Ok(7) }).await.unwrap(), 7);
    }

    #[tokio::test]
    async fn cancelled_leader_fails_the_waiters_instead_of_hanging() {
        let flight = Arc::new(SingleFlight::<u64>::new());
        let (entered_tx, entered_rx) = async_channel::bounded(1);

        // 模拟 Leader 任务中断。
        let leader_flight = flight.clone();
        let leader = tokio::spawn(async move {
            leader_flight
                .run("k", || async {
                    entered_tx.send(()).await.ok();
                    std::future::pending::<()>().await;
                    Ok(1_u64)
                })
                .await
        });

        // 等待 Leader 成功注册在途状态。
        entered_rx.recv().await.unwrap();

        // Follower 登记等待。
        let follower_flight = flight.clone();
        let follower = tokio::spawn(async move { follower_flight.run("k", || async { Ok(2_u64) }).await });
        tokio::time::sleep(Duration::from_millis(50)).await;

        // 中断 Leader 验证 Follower 及时唤醒。
        leader.abort();
        tokio::time::sleep(Duration::from_millis(50)).await;

        let joined = tokio::time::timeout(Duration::from_secs(2), follower)
            .await
            .expect("leader 被取消后 follower 不应挂起")
            .unwrap();
        assert!(joined.is_err(), "follower 应收到明确的失败，而不是永远等待");

        // 验证在途状态清理后允许重新成为 Leader。
        assert_eq!(flight.run("k", || async { Ok(3_u64) }).await.unwrap(), 3);
    }

    #[tokio::test]
    async fn late_arrival_after_completion_gets_result_not_a_hang() {
        let flight: SingleFlight<u64> = SingleFlight::new();
        assert_eq!(flight.run("k", || async { Ok(1) }).await.unwrap(), 1);
        // 任务完成后后续调用重新成为 Leader。
        assert_eq!(flight.run("k", || async { Ok(2) }).await.unwrap(), 2);
    }

    #[tokio::test]
    async fn a_caller_arriving_after_the_leader_finished_reuses_the_published_result() {
        // 回归：结果必须先发布、再摘除在途登记。
        // 否则“leader 已完成、登记尚未摘除”的窗口内的调用方会成为第二个 leader，
        // 重复执行上游请求。
        let flight = Arc::new(SingleFlight::<u64>::new());
        let calls = Arc::new(AtomicU64::new(0));

        // 在发布窗口内发起同键调用：必须拿到已发布的结果，且不得再次执行工作
        let probe_flight = flight.clone();
        let probe_calls = calls.clone();
        flight.set_publish_hook(Box::new(move || {
            let probe_flight = probe_flight.clone();
            let probe_calls = probe_calls.clone();
            Box::pin(async move {
                let value = probe_flight
                    .run("k", || async move {
                        probe_calls.fetch_add(1, Ordering::SeqCst);
                        Ok(999)
                    })
                    .await
                    .unwrap();
                assert_eq!(value, 42, "迟到的调用方必须拿到已发布的结果");
            })
        }));

        let leader_flight = flight.clone();
        let leader_calls = calls.clone();
        let value = leader_flight
            .run("k", || async move {
                leader_calls.fetch_add(1, Ordering::SeqCst);
                Ok(42)
            })
            .await
            .unwrap();
        assert_eq!(value, 42);
        assert_eq!(calls.load(Ordering::SeqCst), 1, "上游请求只能真正执行一次");
    }
}
