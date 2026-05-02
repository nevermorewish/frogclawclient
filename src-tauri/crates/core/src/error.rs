use thiserror::Error;

#[derive(Debug, Error)]
pub enum FrogClawClientError {
    #[error("Database error: {0}")]
    Database(#[from] sea_orm::DbErr),
    #[error("Provider error: {0}")]
    Provider(String),
    #[error("Gateway error: {0}")]
    Gateway(String),
    #[error("Crypto error: {0}")]
    Crypto(String),
    #[error("Not found: {0}")]
    NotFound(String),
    #[error("Validation error: {0}")]
    Validation(String),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

impl serde::Serialize for FrogClawClientError {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl From<sea_orm::TransactionError<sea_orm::DbErr>> for FrogClawClientError {
    fn from(err: sea_orm::TransactionError<sea_orm::DbErr>) -> Self {
        match err {
            sea_orm::TransactionError::Connection(e) => FrogClawClientError::Database(e),
            sea_orm::TransactionError::Transaction(e) => FrogClawClientError::Database(e),
        }
    }
}

pub type Result<T> = std::result::Result<T, FrogClawClientError>;
