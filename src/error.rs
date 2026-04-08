use std::path::PathBuf;
use thiserror::Error;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Error)]
pub enum Error {
    #[error("io error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("messagepack encode/decode error: {0}")]
    MessagePack(#[from] rmp_serde::encode::Error),
    #[error("messagepack decode error: {0}")]
    MessagePackDecode(#[from] rmp_serde::decode::Error),
    #[error("unsupported replay feature: {0}")]
    Unsupported(String),
    #[error("invalid replay: {0}")]
    InvalidReplay(String),
    #[error("invalid sqrb: {0}")]
    InvalidSqrb(String),
    #[error("{0}")]
    Message(String),
}
