//! 单飞并发去重：合并同 key 并发请求并广播执行结果。

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use async_channel::Sender;
use dashmap::DashMap;
use dashmap::mapref::entry::Entry;
use parking_lot::Mutex;

use crate::error::{Error, Result};

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

/// Leader 守卫：在被取消或异常退出时向所有等待者广播失败。
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
        self.owner.inflight.remove(&self.key);
        let error = Error::Transport("single-flight leader 被取消".to_owned());
        // 互斥写入完成状态并转移全部等待通道。
        *self.flight.done.lock() = Some(Err(error.clone()));
        let waiters = std::mem::take(&mut *self.flight.waiters.lock());
        for waiter in waiters {
            // 非阻塞唤醒等待者并派发取消错误。
            let _ = waiter.try_send(Err(error.clone()));
        }
    }
}

/// 并发同 key 请求的合并器。
#[derive(Debug)]
pub struct SingleFlight<T> {
    inflight: DashMap<String, Arc<Flight<T>>>,
    hits: AtomicU64,
}

impl<T> Default for SingleFlight<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> SingleFlight<T> {
    /// 创建单飞合并器实例。
    #[must_use]
    pub fn new() -> Self {
        Self {
            inflight: DashMap::new(),
            hits: AtomicU64::new(0),
        }
    }

    /// 被合并掉的调用次数（可观测性 / 测试用）。
    #[must_use]
    pub fn coalesced(&self) -> u64 {
        self.hits.load(Ordering::Relaxed)
    }
}

impl<T: Clone + Send + Sync + 'static> SingleFlight<T> {
    /// 执行异步任务，并发同键请求共享 leader 执行结果。
    pub async fn run<F, Fut>(&self, key: impl Into<String>, work: F) -> Result<T>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<T>>,
    {
        let key = key.into();

        let (flight, leader, follower_rx) = match self.inflight.entry(key.clone()) {
            Entry::Occupied(occupied) => {
                let flight = occupied.get().clone();
                // 互斥检查已完成状态避免丢失唤醒。
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
                None => unreachable!("非 leader 必然有接收端"),
            };
        }

        // 创建 Leader 保护守卫。
        let mut guard = LeaderGuard {
            owner: self,
            key: key.clone(),
            flight: flight.clone(),
            finished: false,
        };

        let result = work().await;
        guard.finished = true;
        self.inflight.remove(&key);

        *flight.done.lock() = Some(result.clone());
        let waiters = std::mem::take(&mut *flight.waiters.lock());
        for waiter in waiters {
            // 异步广播执行结果至所有等待者。
            let _ = waiter.send(result.clone()).await;
        }
        result
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
}
