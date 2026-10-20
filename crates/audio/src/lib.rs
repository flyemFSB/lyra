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

pub use decoder::{DecodedChunk, Decoder};
pub use engine::{Engine, spawn};
pub use error::Error;
pub use resample::Resampler;
pub use types::{Event, FetchError, FetchResult, Fetcher, ReadSeek, Snapshot, State, TrackRef};
