use std::fmt;

#[derive(Debug)]
pub enum SeparationError {
    ServiceUnavailable,
    InvalidAudio,
    ProcessingFailed(String),
    Timeout,
}

impl fmt::Display for SeparationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ServiceUnavailable => write!(f, "service unavailable"),
            Self::InvalidAudio => write!(f, "invalid audio"),
            Self::ProcessingFailed(msg) => write!(f, "processing failed: {msg}"),
            Self::Timeout => write!(f, "timeout"),
        }
    }
}

impl std::error::Error for SeparationError {}
