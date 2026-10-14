//! 音频引擎共享状态管理（基于原子类型提供无锁读写）。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, Ordering};

use parking_lot::Mutex;

use crate::{State, TrackRef};

pub(crate) struct Shared {
    state: AtomicU8,
    position_ms: AtomicU64,
    duration_ms: AtomicU64,
    volume: AtomicU32,
    /// 缓冲刷新标记（定位时置位，由输出回调消费复位）。
    flush: AtomicBool,
    /// 当前播放曲目引用信息。
    track: Mutex<Option<TrackRef>>,
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
        })
    }

    /// 请求输出回调丢弃环形缓冲区残留音频。
    pub(crate) fn request_flush(&self) {
        self.flush.store(true, Ordering::Release);
    }

    /// 获取并复位缓冲刷新标记，返回是否需要清空缓冲区。
    pub(crate) fn take_flush_request(&self) -> bool {
        self.flush.swap(false, Ordering::AcqRel)
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
