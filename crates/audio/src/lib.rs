//! 音频引擎：流式拉取、解码、重采样与跨线程播放管线。
#![forbid(unsafe_code)]

mod decoder;
mod engine;
mod error;
mod output;
mod resample;
mod ring;
mod state;
mod types;

pub use decoder::{Decoded, DecodedChunk, Decoder, decode_all};
pub use engine::{AudioConfig, Engine, spawn, spawn_with};
pub use error::Error;
pub use resample::{Resampler, resample_all};
pub use types::{Event, FetchError, FetchResult, Fetcher, ReadSeek, Snapshot, State, TrackRef};
