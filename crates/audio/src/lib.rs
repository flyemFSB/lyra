//! 音频引擎核心实现：涵盖流式抓取、格式解码、音频重采样及无锁环形输出管线。
#![forbid(unsafe_code)]

mod decoder;
mod engine;
mod error;
mod output;
mod resample;
mod ring;
mod state;
mod types;

pub use decoder::{Decoded, Decoder, decode_all};
pub use engine::{AudioConfig, Engine, spawn, spawn_with};
pub use error::Error;
pub use resample::{Resampler, resample_all};
pub use types::{Event, FetchError, FetchResult, Fetcher, ReadSeek, Snapshot, State, TrackRef};
