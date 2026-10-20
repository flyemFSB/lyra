//! Lyra音乐传输与端点层：负责端点定义、签名拼装、缓存与离线传输抽象。
#![forbid(unsafe_code)]

pub mod blob;
pub mod cache;
pub mod client;
pub mod codec;
pub mod endpoint;
pub mod error;
pub mod model;
pub mod single_flight;
pub mod spec;
#[cfg(test)]
mod testing;
pub mod transport;

#[cfg(feature = "http")]
pub mod http;

pub use cache::Freshness;
pub use client::Api;
pub use error::{Code, Error, Result};
pub use spec::{Account, RequestSpec, Session};
