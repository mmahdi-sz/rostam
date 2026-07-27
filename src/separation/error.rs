use thiserror::Error;

#[derive(Debug, Error)]
pub enum SeparationError {
    #[error("service unavailable")]
    ServiceUnavailable,
    #[error("invalid audio")]
    InvalidAudio,
    #[error("processing failed: {0}")]
    ProcessingFailed(String),
    #[error("timeout")]
    Timeout,
}

