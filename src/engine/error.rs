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
    #[error("check-and-set parameter required for this call")]
    CasRequired,
    #[error("write queue full — backpressure")]
    QueueFull,
}

impl From<kallisto_kv_model::apply::ModelError> for EngineError {
    fn from(err: kallisto_kv_model::apply::ModelError) -> Self {
        use kallisto_kv_model::apply::ModelError;
        match err {
            ModelError::CasMismatch { expected, actual } => {
                EngineError::CasMismatch { expected, actual }
            }
            ModelError::CasRequired => EngineError::CasRequired,
            ModelError::InvalidVersion(v) => EngineError::InvalidVersion(v),
            ModelError::Destroyed(_) => EngineError::Destroyed,
        }
    }
}
