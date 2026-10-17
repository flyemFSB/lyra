//! 应用层门面：聚合协议、接口、持久化与音频引擎
#![forbid(unsafe_code)]

pub mod cli;
pub mod error;
pub mod fetcher;
pub mod service;

pub use error::{Error, Result};
pub use fetcher::BlobFetcher;
pub use lyra_store::PlayMode;
pub use service::{Kugou, Playback, is_mobile};
