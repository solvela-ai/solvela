use thiserror::Error;

/// Common errors shared across crates.
#[derive(Debug, Error)]
pub enum CommonError {
    #[error("invalid model: {0}")]
    InvalidModel(String),

    #[error("invalid provider: {0}")]
    InvalidProvider(String),

    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("configuration error: {0}")]
    Config(String),
}
