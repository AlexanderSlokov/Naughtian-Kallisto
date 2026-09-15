use async_trait::async_trait;
use rkyv::{Archive, Deserialize, Serialize};
use serde::{Deserialize as SerdeDeserialize, Serialize as SerdeSerialize};

use super::error::EngineError;

#[derive(
    Debug, Clone, SerdeSerialize, SerdeDeserialize, Archive, Serialize, Deserialize, PartialEq, Eq,
)]
pub struct VersionState {
    pub created_time_ms: u64,
    pub deletion_time_ms: u64, // > 0 tức là đã bị Soft-Delete
    pub version_id: u32,
    pub destroyed: bool, // true tức là Payload đã bị wipe
}

#[derive(
    Debug,
    Clone,
    SerdeSerialize,
    SerdeDeserialize,
    Archive,
    Serialize,
    Deserialize,
    PartialEq,
    Eq,
    Default,
)]
pub struct KeyMetadata {
    pub current_version: u32,
    /// Lowest version number still present in `versions`. Retention prunes a
    /// contiguous range of version numbers starting here, so this must be
    /// persisted — recomputing it from `versions` would lose the distinction
    /// between "pruned" and "destroyed".
    pub oldest_version: u32,
    pub max_versions: u32, // 0 = dùng Engine Mount Config mặc định
    pub cas_required: bool,
    pub delete_version_after_ms: u64, // TTL per version
    #[serde(default)]
    pub custom_metadata: std::collections::HashMap<String, String>,
    pub versions: Vec<VersionState>,
}

#[derive(
    Clone, SerdeSerialize, SerdeDeserialize, Archive, Serialize, Deserialize, PartialEq, Eq,
)]
pub struct SecretPayload {
    pub value: String, // Chứa dữ liệu bí mật
    pub ttl: u64,
}

impl std::fmt::Debug for SecretPayload {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SecretPayload")
            .field("value", &"<REDACTED>")
            .field("ttl", &self.ttl)
            .finish()
    }
}

#[async_trait]
pub trait SecretEngine: Send + Sync {
    async fn read_version(
        &self,
        path: &str,
        version: u32,
    ) -> Result<(SecretPayload, VersionState), EngineError>;
    async fn read_metadata(&self, path: &str) -> Result<KeyMetadata, EngineError>;
    async fn put_version(
        &self,
        path: &str,
        payload: &SecretPayload,
        cas: Option<u32>,
    ) -> Result<(), EngineError>;
    async fn soft_delete(&self, path: &str, version: u32) -> Result<(), EngineError>;
    async fn undelete(&self, path: &str, version: u32) -> Result<(), EngineError>;
    async fn destroy_version(&self, path: &str, version: u32) -> Result<(), EngineError>;
    async fn list_keys(&self, prefix: &str) -> Result<Vec<String>, EngineError>;
    fn engine_type(&self) -> &'static str;
    async fn force_flush(&self) -> Result<(), EngineError>;

    /// Switch durability mode: `true` fsyncs before answering, `false` uses
    /// write-behind batching (ADR-0013 D1/D2).
    ///
    /// Defaults to a no-op so engines with a single durability mode need not
    /// implement it. `KvEngine` overrides it — without that override the
    /// `/admin/mode/*` endpoints answered `"OK"` while leaving the mode
    /// untouched, which made D1 impossible to test and the endpoint a lie.
    async fn set_sync_mode(&self, _immediate: bool) -> Result<(), EngineError> {
        Ok(())
    }
}
