use thiserror::Error;

#[allow(dead_code)]
#[derive(Error, Debug)]
pub enum AppError {
    #[error("telegram API error: {0}")]
    Telegram(#[from] frankenstein::Error),

    #[error("database error: {0}")]
    Database(#[from] tokio_postgres::Error),

    #[error("redis error: {0}")]
    Redis(#[from] redis::RedisError),

    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),

    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("{context}: {source}")]
    WithContext {
        context: String,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
}

#[allow(dead_code)]
pub type Result<T> = std::result::Result<T, anyhow::Error>;
