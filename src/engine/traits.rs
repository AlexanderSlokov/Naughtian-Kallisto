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

#[cfg(test)]
mod rkyv_safety {
    //! ADR-0013 C1: `rkyv::archived_root` over the engine's own storage bytes
    //! must not trigger undefined behaviour.
    //!
    //! Run under Miri by `make verify-miri`. The scope is deliberate: these are
    //! archives this crate produced, which is the only case the engine's read
    //! path has a contract for. Resistance to *corrupted* bytes is a separate,
    //! unproven invariant — see `docs/references/verification-status.md`.

    use std::collections::HashMap;

    use super::{KeyMetadata, SecretPayload, VersionState};

    fn sample_metadata() -> KeyMetadata {
        KeyMetadata {
            current_version: 9,
            oldest_version: 4,
            max_versions: 6,
            cas_required: true,
            delete_version_after_ms: 86_400_000,
            custom_metadata: HashMap::from([
                ("owner".to_string(), "platform".to_string()),
                ("rotation".to_string(), "quarterly".to_string()),
            ]),
            versions: (4..=9)
                .map(|id| VersionState {
                    created_time_ms: 1_700_000_000_000 + u64::from(id),
                    deletion_time_ms: if id % 2 == 0 { 0 } else { 1_800_000_000_000 },
                    version_id: id,
                    destroyed: id == 5,
                })
                .collect(),
        }
    }

    #[test]
    fn c1_metadata_archive_reads_back_without_ub() {
        let meta = sample_metadata();
        let bytes = rkyv::to_bytes::<_, 256>(&meta).unwrap();

        // SAFETY: `bytes` is an archive produced immediately above by the same
        // rkyv version for the same type, and `AlignedVec` satisfies the
        // alignment `archived_root` requires. This mirrors the engine's read
        // path exactly.
        let archived = unsafe { rkyv::archived_root::<KeyMetadata>(&bytes) };

        assert_eq!(archived.current_version, meta.current_version);
        assert_eq!(archived.oldest_version, meta.oldest_version);
        assert_eq!(archived.cas_required, meta.cas_required);
        assert_eq!(archived.versions.len(), meta.versions.len());
        // Touch every field of every element: Miri only sees the loads that
        // actually happen, so a test that reads one field proves one field.
        for (got, want) in archived.versions.iter().zip(&meta.versions) {
            assert_eq!(got.version_id, want.version_id);
            assert_eq!(got.created_time_ms, want.created_time_ms);
            assert_eq!(got.deletion_time_ms, want.deletion_time_ms);
            assert_eq!(got.destroyed, want.destroyed);
        }
    }

    #[test]
    fn c1_payload_archive_reads_back_without_ub() {
        let payload = SecretPayload {
            value: "a".repeat(600),
            ttl: 3600,
        };
        let bytes = rkyv::to_bytes::<_, 256>(&payload).unwrap();

        // SAFETY: as above.
        let archived = unsafe { rkyv::archived_root::<SecretPayload>(&bytes) };
        assert_eq!(archived.value.as_str(), payload.value);
        assert_eq!(archived.ttl, payload.ttl);
    }

    #[test]
    fn c1_empty_collections_archive_cleanly() {
        // Zero-length vec and map are the shapes most likely to produce a
        // dangling relative pointer.
        let meta = KeyMetadata::default();
        let bytes = rkyv::to_bytes::<_, 256>(&meta).unwrap();

        // SAFETY: as above.
        let archived = unsafe { rkyv::archived_root::<KeyMetadata>(&bytes) };
        assert_eq!(archived.versions.len(), 0);
        assert_eq!(archived.current_version, 0);
        assert!(archived.versions.iter().next().is_none());
    }
}
