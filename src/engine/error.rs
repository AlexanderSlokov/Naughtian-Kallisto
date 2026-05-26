#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EngineError {
    #[error("secret not found")]
    NotFound,
    #[error("version soft-deleted")]
    SoftDeleted,
    #[error("version permanently destroyed")]
    Destroyed,
    #[error("storage backend error: {0}")]
    StorageError(String),
    #[error("invalid version: {0}")]
    InvalidVersion(u32),
    #[error("CAS mismatch: expected {expected}, got {actual}")]
    CasMismatch { expected: u32, actual: u32 },
    #[error("write queue full — backpressure")]
    QueueFull,
}
