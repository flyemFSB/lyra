//! 引擎跨线程共享状态（基于原子操作无锁读写）。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, Ordering};

use parking_lot::Mutex;

use crate::{State, TrackRef};

pub(crate) struct Shared {
    state: AtomicU8,
    position_ms: AtomicU64,
    duration_ms: AtomicU64,
    volume: AtomicU32,
    /// 缓冲刷新标记（Seek 时置位，回调清空后复位）
    flush: AtomicBool,
    /// 当前曲目信息
    track: Mutex<Option<TrackRef>>,
    /// 输出流错误（实时线程写入，非实时线程读取并记录日志）
    stream_error: Mutex<Option<String>>,
}

fn state_to_u8(state: State) -> u8 {
    match state {
        State::Idle => 0,
        State::Loading => 1,
        State::Playing => 2,
        State::Paused => 3,
    }
}

fn state_from_u8(v: u8) -> State {
    match v {
        1 => State::Loading,
        2 => State::Playing,
        3 => State::Paused,
        _ => State::Idle,
    }
}

impl Shared {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            state: AtomicU8::new(state_to_u8(State::Idle)),
            position_ms: AtomicU64::new(0),
            duration_ms: AtomicU64::new(0),
            volume: AtomicU32::new(1.0f32.to_bits()),
            flush: AtomicBool::new(false),
            track: Mutex::new(None),
            stream_error: Mutex::new(None),
        })
    }

    /// 请求清空环形缓冲区残留样本
    pub(crate) fn request_flush(&self) {
        self.flush.store(true, Ordering::Release);
    }

    /// 读取并复位刷新标记（仅回调侧可消费）
    pub(crate) fn take_flush_request(&self) -> bool {
        self.flush.swap(false, Ordering::AcqRel)
    }

    /// 查询刷新请求是否仍待处理（生产者侧只读观察）
    ///
    /// 解码线程在写入样本前必须先看这个标志：回调清空环形缓冲与生产者写入之间
    /// 必须有确定的先后顺序，否则刚推入的新样本会被回调一起清掉（每首歌开头被削）。
    pub(crate) fn flush_pending(&self) -> bool {
        self.flush.load(Ordering::Acquire)
    }

    /// 记录输出流错误（由实时回调线程调用）
    ///
    /// 实时线程不能调用 `tracing`（会格式化并抢全局锁，造成优先级反转），
    /// 因此这里只做一次 `try_lock` 写入，真正的日志由非实时线程打出。
    /// 锁被占用时直接丢弃这条错误信息，绝不在实时线程上阻塞。
    pub(crate) fn record_stream_error(&self, message: String) {
        if let Some(mut slot) = self.stream_error.try_lock() {
            *slot = Some(message);
        }
    }

    /// 取出并清除已记录的输出流错误（由非实时线程调用）
    pub(crate) fn take_stream_error(&self) -> Option<String> {
        self.stream_error.lock().take()
    }

    pub(crate) fn state(&self) -> State {
        state_from_u8(self.state.load(Ordering::Relaxed))
    }

    pub(crate) fn set_state(&self, state: State) {
        self.state.store(state_to_u8(state), Ordering::Relaxed);
    }

    pub(crate) fn is_playing(&self) -> bool {
        self.state.load(Ordering::Relaxed) == state_to_u8(State::Playing)
    }

    pub(crate) fn volume(&self) -> f32 {
        f32::from_bits(self.volume.load(Ordering::Relaxed))
    }

    pub(crate) fn set_volume(&self, volume: f32) {
        self.volume.store(volume.to_bits(), Ordering::Relaxed);
    }

    pub(crate) fn track(&self) -> Option<TrackRef> {
        self.track.lock().clone()
    }

    pub(crate) fn set_track(&self, track: TrackRef) {
        *self.track.lock() = Some(track);
    }

    pub(crate) fn position_ms(&self) -> u64 {
        self.position_ms.load(Ordering::Relaxed)
    }

    pub(crate) fn duration_ms(&self) -> u64 {
        self.duration_ms.load(Ordering::Relaxed)
    }

    pub(crate) fn set_position(&self, position_ms: u64, duration_ms: u64) {
        self.position_ms.store(position_ms, Ordering::Relaxed);
        self.duration_ms.store(duration_ms, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::pedantic)]

    use super::*;

    #[test]
    fn flush_request_is_consumed_exactly_once() {
        let shared = Shared::new();
        assert!(!shared.take_flush_request());
        shared.request_flush();
        assert!(shared.take_flush_request());
        assert!(
            !shared.take_flush_request(),
            "取走之后不应重复触发，否则实时回调会反复清空 ring"
        );
    }

    #[test]
    fn a_pending_flush_is_visible_to_the_producer() {
        // 刷新握手：生产者必须能看到"回调还没清空"这个状态，
        // 否则它会在回调清空之前把新样本写进去，随之一起被丢掉。
        let shared = Shared::new();
        assert!(!shared.flush_pending());
        shared.request_flush();
        assert!(shared.flush_pending(), "生产者必须能观察到待处理的刷新请求");
        // 回调消费掉之后，生产者才允许继续写入
        assert!(shared.take_flush_request());
        assert!(!shared.flush_pending());
    }

    #[test]
    fn stream_errors_are_recorded_once_and_taken_out() {
        let shared = Shared::new();
        assert!(shared.take_stream_error().is_none());
        shared.record_stream_error("boom".to_owned());
        assert_eq!(shared.take_stream_error().as_deref(), Some("boom"));
        assert!(shared.take_stream_error().is_none(), "取出后不应重复上报");
    }

    #[test]
    fn state_and_volume_round_trip() {
        let shared = Shared::new();
        assert_eq!(shared.state(), State::Idle);
        shared.set_state(State::Playing);
        assert!(shared.is_playing());
        shared.set_volume(0.25);
        assert!((shared.volume() - 0.25).abs() < f32::EPSILON);
        shared.set_position(1234, 5678);
        assert_eq!(shared.position_ms(), 1234);
        assert_eq!(shared.duration_ms(), 5678);
    }
}
