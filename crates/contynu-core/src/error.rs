use thiserror::Error;

pub type Result<T> = std::result::Result<T, ContynuError>;

#[derive(Debug, Error)]
pub enum ContynuError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("invalid state: {0}")]
    InvalidState(String),
}
