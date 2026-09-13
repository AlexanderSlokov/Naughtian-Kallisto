use crate::{effects::Effect, ops::KvOp};

/// KV-v2 version state, matching the engine's `VersionState`.
/// Duplicated here to keep this crate free of I/O and serialization deps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionState {
    pub created_time_ms: u64,
    /// `> 0` means soft-deleted at this timestamp.
    pub deletion_time_ms: u64,
    pub version_id: u32,
    /// `true` means payload permanently destroyed.
    pub destroyed: bool,
}

/// KV-v2 key metadata, matching the engine's `KeyMetadata`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct KeyMetadata {
    pub current_version: u32,
    /// 0 = use engine mount config default (no trim).
    pub max_versions: u32,
    pub cas_required: bool,
    pub delete_version_after_ms: u64,
    pub versions: Vec<VersionState>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ModelError {
    #[error("CAS mismatch: expected {expected}, got {actual}")]
    CasMismatch { expected: u32, actual: u32 },
    #[error("version {0} not found in metadata")]
    InvalidVersion(u32),
    #[error("version {0} is permanently destroyed")]
    Destroyed(u32),
}

/// Single pure state transition for KV-v2 semantics.
///
/// Given the current metadata, an operation, and the current timestamp,
/// returns the new metadata and a list of side-effects the engine must
/// execute against I/O. Identical inputs always produce identical outputs.
///
/// # Example
/// ```
/// use kallisto_kv_model::{
///     apply::{KeyMetadata, ModelError, apply},
///     ops::KvOp,
/// };
///
/// let meta = KeyMetadata::default();
/// let (new_meta, effects) = apply(
///     &meta,
///     KvOp::Put {
///         payload_len: 42,
///         cas: None,
///     },
///     1000,
/// )
/// .unwrap();
/// assert_eq!(new_meta.current_version, 1);
/// ```
pub fn apply(
    meta: &KeyMetadata,
    op: KvOp,
    now_ms: u64,
) -> Result<(KeyMetadata, Vec<Effect>), ModelError> {
    match op {
        KvOp::Put { cas, .. } => apply_put(meta, cas, now_ms),
        KvOp::SoftDelete { version } => apply_soft_delete(meta, version, now_ms),
        KvOp::Undelete { version } => apply_undelete(meta, version),
        KvOp::Destroy { version } => apply_destroy(meta, version),
        KvOp::UpdateMeta {
            max_versions,
            cas_required,
            delete_version_after_ms,
        } => apply_update_meta(meta, max_versions, cas_required, delete_version_after_ms),
    }
}

fn apply_put(
    meta: &KeyMetadata,
    cas: Option<u32>,
    now_ms: u64,
) -> Result<(KeyMetadata, Vec<Effect>), ModelError> {
    // A4: CAS mismatch leaves state byte-for-byte unchanged.
    if let Some(expected_cas) = cas
        && meta.current_version != expected_cas
    {
        return Err(ModelError::CasMismatch {
            expected: expected_cas,
            actual: meta.current_version,
        });
    }

    let mut new_meta = meta.clone();
    // A1: current_version is monotonically increasing.
    new_meta.current_version += 1;

    let vs = VersionState {
        version_id: new_meta.current_version,
        created_time_ms: now_ms,
        deletion_time_ms: 0,
        destroyed: false,
    };
    // A2: versions ordered ascending by version_id, no duplicates.
    new_meta.versions.push(vs);

    let mut effects = vec![
        Effect::WriteVersion {
            version: new_meta.current_version,
        },
        Effect::WriteMeta,
        Effect::IndexPath,
    ];

    // A7: enforce max_versions trim. When max_versions > 0, purge the oldest
    // non-destroyed version if we exceed the limit. Matches Vault behavior in
    // hashicorp/vault/builtin/logical/kv/path_data.go (~line 300).
    if new_meta.max_versions > 0 {
        while new_meta.versions.len() as u32 > new_meta.max_versions {
            if let Some(pos) = new_meta.versions.iter().position(|v| !v.destroyed) {
                let trimmed = new_meta.versions.remove(pos);
                effects.push(Effect::TrimVersion {
                    version: trimmed.version_id,
                });
            } else {
                // All remaining versions are destroyed; nothing more to trim.
                break;
            }
        }
    }

    Ok((new_meta, effects))
}

fn apply_soft_delete(
    meta: &KeyMetadata,
    version: u32,
    now_ms: u64,
) -> Result<(KeyMetadata, Vec<Effect>), ModelError> {
    let mut new_meta = meta.clone();
    let vs = find_version_mut(&mut new_meta.versions, version)?;
    vs.deletion_time_ms = now_ms;
    Ok((new_meta, vec![Effect::WriteMeta]))
}

fn apply_undelete(
    meta: &KeyMetadata,
    version: u32,
) -> Result<(KeyMetadata, Vec<Effect>), ModelError> {
    let mut new_meta = meta.clone();
    let vs = find_version_mut(&mut new_meta.versions, version)?;
    // A3: destroyed = true is terminal. Undelete on a destroyed version is an
    // error.
    if vs.destroyed {
        return Err(ModelError::Destroyed(version));
    }
    // A6: undelete ∘ soft_delete = identity for non-destroyed versions.
    vs.deletion_time_ms = 0;
    Ok((new_meta, vec![Effect::WriteMeta]))
}

fn apply_destroy(
    meta: &KeyMetadata,
    version: u32,
) -> Result<(KeyMetadata, Vec<Effect>), ModelError> {
    let mut new_meta = meta.clone();
    let vs = find_version_mut(&mut new_meta.versions, version)?;
    // A3: destroyed is terminal.
    // A8: destroy deletes payload but retains VersionState in metadata.
    vs.destroyed = true;
    Ok((
        new_meta,
        vec![Effect::DeleteVersion { version }, Effect::WriteMeta],
    ))
}

fn apply_update_meta(
    meta: &KeyMetadata,
    max_versions: u32,
    cas_required: bool,
    delete_version_after_ms: u64,
) -> Result<(KeyMetadata, Vec<Effect>), ModelError> {
    let mut new_meta = meta.clone();
    new_meta.max_versions = max_versions;
    new_meta.cas_required = cas_required;
    new_meta.delete_version_after_ms = delete_version_after_ms;
    Ok((new_meta, vec![Effect::WriteMeta]))
}

fn find_version_mut(
    versions: &mut [VersionState],
    version_id: u32,
) -> Result<&mut VersionState, ModelError> {
    versions
        .iter_mut()
        .find(|v| v.version_id == version_id)
        .ok_or(ModelError::InvalidVersion(version_id))
}

/// Build a storage key for metadata that is injective over all valid paths.
///
/// A9 fix: uses length-prefix encoding to prevent key collisions when paths
/// contain `:`. Format: `m:{path_byte_len_hex8}:{path}`.
///
/// # Example
/// ```
/// use kallisto_kv_model::apply::build_meta_key;
/// assert_eq!(build_meta_key("app/db"), "m:00000006:app/db");
/// ```
pub fn build_meta_key(path: &str) -> String {
    format!("m:{:08x}:{}", path.len(), path)
}

/// Build a storage key for a versioned payload that is injective over
/// all `(path, version)` pairs.
///
/// A9 fix: uses length-prefix encoding to prevent key collisions when paths
/// contain `:`. Format: `v:{path_byte_len_hex8}:{path}:{version}`.
///
/// # Example
/// ```
/// use kallisto_kv_model::apply::build_version_key;
/// assert_eq!(build_version_key("app/db", 3), "v:00000006:app/db:3");
/// // Paths containing `:` do not collide:
/// assert_ne!(build_version_key("a:b", 1), build_version_key("a", 0));
/// ```
pub fn build_version_key(path: &str, version: u32) -> String {
    format!("v:{:08x}:{}:{}", path.len(), path, version)
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;
    use crate::ops::KvOp;

    // --- Strategies ---

    fn arb_put() -> impl Strategy<Value = KvOp> {
        (0usize..1024, proptest::option::of(0u32..100)).prop_map(|(len, cas)| KvOp::Put {
            payload_len: len,
            cas,
        })
    }

    fn arb_op() -> impl Strategy<Value = KvOp> {
        prop_oneof![
            arb_put(),
            (1u32..20).prop_map(|v| KvOp::SoftDelete { version: v }),
            (1u32..20).prop_map(|v| KvOp::Undelete { version: v }),
            (1u32..20).prop_map(|v| KvOp::Destroy { version: v }),
        ]
    }

    fn arb_op_sequence() -> impl Strategy<Value = Vec<KvOp>> {
        proptest::collection::vec(arb_op(), 1..30)
    }

    /// Apply a sequence of operations, ignoring errors (they are expected
    /// for invalid version refs, CAS mismatches, etc.). Returns the final
    /// metadata after all operations.
    fn apply_sequence(ops: &[KvOp]) -> KeyMetadata {
        let mut meta = KeyMetadata::default();
        for (time, op) in (1000u64..).zip(ops.iter().cloned()) {
            if let Ok((new_meta, _)) = apply(&meta, op, time) {
                meta = new_meta;
            }
        }
        meta
    }

    // --- A1: current_version monotonically increasing ---

    proptest! {
        #[test]
        fn prop_current_version_monotone(ops in arb_op_sequence()) {
            let mut meta = KeyMetadata::default();
            let mut time = 1000u64;
            let mut prev_version = meta.current_version;
            for op in &ops {
                if let Ok((new_meta, _)) = apply(&meta, op.clone(), time) {
                    prop_assert!(
                        new_meta.current_version >= prev_version,
                        "current_version decreased from {} to {}",
                        prev_version,
                        new_meta.current_version,
                    );
                    prev_version = new_meta.current_version;
                    meta = new_meta;
                }
                time += 1;
            }
        }
    }

    // --- A2: versions ordered ascending, no duplicates ---

    proptest! {
        #[test]
        fn prop_versions_sorted_no_dups(ops in arb_op_sequence()) {
            let meta = apply_sequence(&ops);
            for window in meta.versions.windows(2) {
                prop_assert!(
                    window[0].version_id < window[1].version_id,
                    "versions not strictly ascending: {:?} >= {:?}",
                    window[0].version_id,
                    window[1].version_id,
                );
            }
        }
    }

    // --- A3: destroyed = true is terminal ---

    proptest! {
        #[test]
        fn prop_destroyed_is_terminal(ops in arb_op_sequence()) {
            let mut meta = KeyMetadata::default();
            let mut time = 1000u64;
            let mut destroyed_versions = std::collections::HashSet::new();

            for op in &ops {
                if let Ok((new_meta, _)) = apply(&meta, op.clone(), time) {
                    // Record any newly destroyed versions.
                    for vs in &new_meta.versions {
                        if vs.destroyed {
                            destroyed_versions.insert(vs.version_id);
                        }
                    }
                    // Verify no previously destroyed version became un-destroyed.
                    for vid in &destroyed_versions {
                        if let Some(vs) = new_meta.versions.iter().find(|v| v.version_id == *vid) {
                            prop_assert!(
                                vs.destroyed,
                                "version {} was un-destroyed after being destroyed",
                                vid,
                            );
                        }
                        // If the version was trimmed out entirely, that's fine—
                        // the version is gone, not un-destroyed.
                    }
                    meta = new_meta;
                }
                time += 1;
            }
        }
    }

    // --- A4: CAS mismatch leaves state unchanged ---

    proptest! {
        #[test]
        fn prop_cas_mismatch_no_mutation(version in 0u32..50) {
            let mut meta = KeyMetadata::default();
            // Put a few versions first.
            for i in 0..3 {
                let (new_meta, _) = apply(
                    &meta,
                    KvOp::Put { payload_len: 10, cas: None },
                    1000 + i,
                ).unwrap();
                meta = new_meta;
            }
            // current_version is now 3. Use a wrong CAS value.
            let wrong_cas = if version == meta.current_version {
                version.wrapping_add(1)
            } else {
                version
            };
            let before = meta.clone();
            let result = apply(&meta, KvOp::Put { payload_len: 10, cas: Some(wrong_cas) }, 2000);
            prop_assert!(result.is_err());
            prop_assert_eq!(&meta, &before, "CAS mismatch must not mutate state");
        }
    }

    // --- A5: put then read round-trip (verified at model level) ---

    proptest! {
        #[test]
        fn prop_put_creates_version(_payload_len in 1usize..1024) {
            let meta = KeyMetadata::default();
            let (new_meta, effects) = apply(
                &meta,
                KvOp::Put { payload_len: _payload_len, cas: None },
                5000,
            ).unwrap();
            prop_assert_eq!(new_meta.current_version, 1);
            // The version exists in metadata.
            prop_assert!(
                new_meta.versions.iter().any(|v| v.version_id == 1),
                "put must create version 1 in metadata"
            );
            // A WriteVersion effect was emitted.
            prop_assert!(
                effects.iter().any(|e| matches!(e, Effect::WriteVersion { version: 1 })),
                "put must emit WriteVersion effect"
            );
        }
    }

    // --- A6: undelete ∘ soft_delete = identity for non-destroyed ---

    proptest! {
        #[test]
        fn prop_undelete_soft_delete_identity(n_puts in 1u32..5) {
            let mut meta = KeyMetadata::default();
            for i in 0..n_puts {
                let (m, _) = apply(&meta, KvOp::Put { payload_len: 10, cas: None }, 1000 + u64::from(i)).unwrap();
                meta = m;
            }
            let target_version = 1u32;
            let before_delete = meta.clone();

            // soft_delete
            let (after_delete, _) = apply(&meta, KvOp::SoftDelete { version: target_version }, 2000).unwrap();
            // undelete
            let (after_undelete, _) = apply(&after_delete, KvOp::Undelete { version: target_version }, 2001).unwrap();

            // The version state should be identical to before the soft_delete,
            // except deletion_time_ms is zeroed (same as before_delete).
            let before_vs = before_delete.versions.iter().find(|v| v.version_id == target_version).unwrap();
            let after_vs = after_undelete.versions.iter().find(|v| v.version_id == target_version).unwrap();
            prop_assert_eq!(before_vs.deletion_time_ms, after_vs.deletion_time_ms);
            prop_assert_eq!(before_vs.destroyed, after_vs.destroyed);
        }
    }

    // --- A7: versions.len() <= max_versions after trimming ---

    proptest! {
        #[test]
        fn prop_max_versions_enforced(n_puts in 3u32..20, max_v in 1u32..5) {
            let mut meta = KeyMetadata {
                max_versions: max_v,
                ..KeyMetadata::default()
            };
            for i in 0..n_puts {
                let (m, _) = apply(
                    &meta,
                    KvOp::Put { payload_len: 10, cas: None },
                    1000 + u64::from(i),
                ).unwrap();
                meta = m;
                prop_assert!(
                    meta.versions.len() as u32 <= max_v,
                    "versions.len()={} exceeds max_versions={} after put #{}",
                    meta.versions.len(),
                    max_v,
                    i + 1,
                );
            }
        }
    }

    // --- A8: destroy deletes payload but retains VersionState ---

    #[test]
    fn destroy_retains_version_state() {
        let mut meta = KeyMetadata::default();
        let (m, _) = apply(
            &meta,
            KvOp::Put {
                payload_len: 10,
                cas: None,
            },
            1000,
        )
        .unwrap();
        meta = m;

        let (after_destroy, effects) = apply(&meta, KvOp::Destroy { version: 1 }, 2000).unwrap();

        // VersionState still exists in metadata.
        let vs = after_destroy
            .versions
            .iter()
            .find(|v| v.version_id == 1)
            .expect("VersionState must be retained after destroy");
        assert!(vs.destroyed);

        // A DeleteVersion effect was emitted (payload deletion).
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::DeleteVersion { version: 1 }))
        );
    }

    // --- A9: (path, version) -> storage key is injective ---

    proptest! {
        #[test]
        fn prop_storage_key_injective(
            path_a in "[a-z:./]{1,20}",
            path_b in "[a-z:./]{1,20}",
            ver_a in 0u32..100,
            ver_b in 0u32..100,
        ) {
            if (path_a.as_str(), ver_a) != (path_b.as_str(), ver_b) {
                let key_a = build_version_key(&path_a, ver_a);
                let key_b = build_version_key(&path_b, ver_b);
                prop_assert_ne!(
                    key_a.clone(), key_b,
                    "collision: ({:?}, {}) and ({:?}, {}) both generated key {}",
                    path_a, ver_a, path_b, ver_b, key_a,
                );
            }
            // Also check meta keys.
            if path_a != path_b {
                let mk_a = build_meta_key(&path_a);
                let mk_b = build_meta_key(&path_b);
                prop_assert_ne!(mk_a, mk_b);
            }
        }
    }

    // --- Deterministic unit tests for key encoding ---

    #[test]
    fn key_encoding_format() {
        assert_eq!(build_meta_key("app/db"), "m:00000006:app/db");
        assert_eq!(build_version_key("app/db", 3), "v:00000006:app/db:3");
    }

    #[test]
    fn key_encoding_colon_path_no_collision() {
        // The old format `v:{path}:{version}` would produce identical keys for:
        //   ("a:b", 1) -> "v:a:b:1"
        //   ("a",  "b:1" parsed somehow)
        // Length-prefix prevents this.
        let k1 = build_version_key("a:b", 1);
        let k2 = build_version_key("a", 1);
        assert_ne!(k1, k2);
    }
}
