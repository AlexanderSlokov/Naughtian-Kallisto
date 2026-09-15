//! Pure KV-v2 state transitions.
//!
//! This is Kallisto's own specification of KV-v2 key state. The rules below are
//! the contract this engine implements, written here as the single place they
//! live, and they are stated in terms of the observable HTTP API that clients
//! and Terraform already speak — not in terms of any other implementation's
//! internals.
//!
//! `tests/e2e_vault_compat.rs` is what anchors these rules to external reality.
//! See ADR-0013's note on the trusted computing base for why proving this
//! module self-consistent is not the same as proving it compatible.
//!
//! The rules, listed because each one has been got wrong here at least once:
//!
//! 1. Version numbers only ever increase, and are never reused.
//! 2. A write past the retention limit prunes a *contiguous range* of the
//!    oldest version numbers. Destroyed versions occupy a slot in that window
//!    like any other; they are not skipped.
//! 3. The version a write just created is never pruned by that same write.
//! 4. `max_versions == 0` means "use [`DEFAULT_MAX_VERSIONS`]", not "keep
//!    everything".
//! 5. Lowering the retention limit does not prune retroactively. The next write
//!    catches up, in one pass.
//! 6. `cas_required` rejects a write carrying no `cas` at all — separately from
//!    a `cas` that fails to match.
//! 7. `delete_version_after` stamps a *future* expiry on a version at write
//!    time, so `deletion_time_ms` is a timestamp, not a flag. See
//!    [`is_readable`].
//! 8. Soft-delete, undelete and destroy skip a version that is missing or
//!    already in the target state, and report success rather than an error.
//! 9. Destruction is terminal: undelete must not resurrect a destroyed version.

use crate::{effects::Effect, ops::KvOp};

/// Versions retained when neither the mount config nor the key's metadata
/// sets `max_versions`. The previous model treated 0 as "never
/// trim", which let metadata grow without bound.
pub const DEFAULT_MAX_VERSIONS: u32 = 10;

/// KV-v2 version state, mirroring the engine's `VersionState`.
/// Duplicated here to keep this crate free of I/O and serialization deps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionState {
    pub created_time_ms: u64,
    /// Absolute timestamp, not a flag. `0` means "not scheduled for deletion".
    /// A value in the future comes from `delete_version_after` and means the
    /// version is still readable until then; a value at or before `now` means
    /// deleted. See [`is_readable`].
    pub deletion_time_ms: u64,
    pub version_id: u32,
    /// `true` means payload permanently destroyed.
    pub destroyed: bool,
}

/// KV-v2 key metadata, mirroring the engine's `KeyMetadata`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct KeyMetadata {
    pub current_version: u32,
    /// Lowest version number still present in `versions`.
    ///
    /// Retention is a function of this and `current_version` — a range over
    /// version numbers — not of the vector's contents. That is what makes it
    /// independent of whether a version happens to be destroyed.
    pub oldest_version: u32,
    /// `0` = use [`DEFAULT_MAX_VERSIONS`].
    pub max_versions: u32,
    pub cas_required: bool,
    /// `0` = disabled. Otherwise each new version gets
    /// `deletion_time_ms = now + delete_version_after_ms`.
    pub delete_version_after_ms: u64,
    pub versions: Vec<VersionState>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ModelError {
    #[error("CAS mismatch: expected {expected}, got {actual}")]
    CasMismatch { expected: u32, actual: u32 },
    #[error("check-and-set parameter required for this call")]
    CasRequired,
    #[error("version {0} not found in metadata")]
    InvalidVersion(u32),
    #[error("version {0} is permanently destroyed")]
    Destroyed(u32),
}

/// Is this version's payload readable at `now_ms`?
///
/// A version is unreadable once destroyed, or once its deletion time is set
/// and has passed. A `deletion_time_ms` in the future is
/// a pending `delete_version_after` expiry and does *not* hide the version.
#[must_use]
pub fn is_readable(vs: &VersionState, now_ms: u64) -> bool {
    !vs.destroyed && (vs.deletion_time_ms == 0 || vs.deletion_time_ms > now_ms)
}

/// `max_versions` in force: the key's own value, else the default.
#[must_use]
pub fn effective_max_versions(meta: &KeyMetadata) -> u32 {
    if meta.max_versions > 0 {
        meta.max_versions
    } else {
        DEFAULT_MAX_VERSIONS
    }
}

/// Single pure state transition for KV-v2 semantics.
///
/// Given the current metadata, an operation, and the current timestamp, returns
/// the new metadata and the side-effects the engine must execute against I/O.
/// Identical inputs always produce identical outputs.
///
/// Operations with nothing to do (soft-deleting a destroyed version, undeleting
/// a version that is gone) return `Ok` with an empty effect list and unchanged
/// metadata, so the handler answers 204 rather than an error.
///
/// # Example
/// ```
/// use kallisto_kv_model::{
///     apply::{KeyMetadata, apply},
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
/// assert!(!effects.is_empty());
/// ```
pub fn apply(
    meta: &KeyMetadata,
    op: KvOp,
    now_ms: u64,
) -> Result<(KeyMetadata, Vec<Effect>), ModelError> {
    match op {
        KvOp::Put { cas, .. } => apply_put(meta, cas, now_ms),
        KvOp::SoftDelete { version } => apply_soft_delete(meta, version, now_ms),
        KvOp::Undelete { version } => apply_undelete(meta, version, now_ms),
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
    // Both CAS branches must run before any mutation, so that A4 — a rejected
    // write leaves state untouched byte-for-byte — holds.
    match cas {
        Some(expected) if meta.current_version != expected => {
            return Err(ModelError::CasMismatch {
                expected,
                actual: meta.current_version,
            });
        }
        // No `cas` supplied at all while the key demands one. Distinct from a
        // mismatch, and the case the previous model ignored outright.
        None if meta.cas_required => return Err(ModelError::CasRequired),
        _ => {}
    }

    let mut new_meta = meta.clone();
    // A1: current_version is monotonically increasing.
    new_meta.current_version += 1;

    // `delete_version_after` stamps a future expiry on the new version at write
    // time. 0 leaves the version with no expiry.
    let deletion_time_ms = if meta.delete_version_after_ms > 0 {
        now_ms.saturating_add(meta.delete_version_after_ms)
    } else {
        0
    };

    // A2: versions stay ordered ascending by version_id, no duplicates —
    // current_version only ever grows, so pushing keeps the order.
    new_meta.versions.push(VersionState {
        version_id: new_meta.current_version,
        created_time_ms: now_ms,
        deletion_time_ms,
        destroyed: false,
    });

    let mut effects = vec![
        Effect::WriteVersion {
            version: new_meta.current_version,
        },
        Effect::WriteMeta,
        Effect::IndexPath,
    ];

    // A7. Retention is a range operation over [oldest_version,
    // version_to_delete], driven purely by version numbers. Destroyed versions
    // are NOT skipped — they hold a slot until the range sweeps past them — and
    // the version just written is never inside the range.
    let max_versions = effective_max_versions(&new_meta);
    if new_meta
        .current_version
        .saturating_sub(new_meta.oldest_version)
        >= max_versions
    {
        let version_to_delete = new_meta.current_version - max_versions;
        // A loop, not a single delete: `max_versions` may have been lowered
        // since the last write, so this write has to catch up.
        for victim in new_meta.oldest_version..=version_to_delete {
            if let Some(pos) = new_meta
                .versions
                .iter()
                .position(|v| v.version_id == victim)
            {
                new_meta.versions.remove(pos);
                effects.push(Effect::TrimVersion { version: victim });
            }
        }
        new_meta.oldest_version = version_to_delete + 1;
    }

    Ok((new_meta, effects))
}

fn apply_soft_delete(
    meta: &KeyMetadata,
    version: u32,
    now_ms: u64,
) -> Result<(KeyMetadata, Vec<Effect>), ModelError> {
    // A missing or destroyed version is skipped, and so is one whose deletion
    // time has already passed. All three are successes with nothing to write,
    // not errors. A3 holds because a destroyed version is never touched.
    let Some(vs) = meta.versions.iter().find(|v| v.version_id == version) else {
        return Ok((meta.clone(), Vec::new()));
    };
    if vs.destroyed || (vs.deletion_time_ms > 0 && vs.deletion_time_ms <= now_ms) {
        return Ok((meta.clone(), Vec::new()));
    }

    let mut new_meta = meta.clone();
    find_version_mut(&mut new_meta.versions, version)?.deletion_time_ms = now_ms;
    Ok((new_meta, vec![Effect::WriteMeta]))
}

fn apply_undelete(
    meta: &KeyMetadata,
    version: u32,
    now_ms: u64,
) -> Result<(KeyMetadata, Vec<Effect>), ModelError> {
    // A destroyed version is a silent no-op, not an error: A3 is upheld by
    // refusing to resurrect it, not by failing the request.
    let Some(vs) = meta.versions.iter().find(|v| v.version_id == version) else {
        return Ok((meta.clone(), Vec::new()));
    };
    if vs.destroyed {
        return Ok((meta.clone(), Vec::new()));
    }

    let mut new_meta = meta.clone();
    // Clear the deletion time, then re-arm `delete_version_after` if it is
    // configured. A6 (undelete ∘ soft_delete =
    // identity) therefore holds exactly when the TTL is disabled; with a TTL the
    // version returns with a fresh expiry rather than its original one.
    let dva = meta.delete_version_after_ms;
    let vs = find_version_mut(&mut new_meta.versions, version)?;
    vs.deletion_time_ms = if dva > 0 {
        now_ms.saturating_add(dva)
    } else {
        0
    };
    Ok((new_meta, vec![Effect::WriteMeta]))
}

fn apply_destroy(
    meta: &KeyMetadata,
    version: u32,
) -> Result<(KeyMetadata, Vec<Effect>), ModelError> {
    // Missing or already-destroyed versions are skipped. Re-destroying must not
    // re-emit a payload delete.
    let Some(vs) = meta.versions.iter().find(|v| v.version_id == version) else {
        return Ok((meta.clone(), Vec::new()));
    };
    if vs.destroyed {
        return Ok((meta.clone(), Vec::new()));
    }

    let mut new_meta = meta.clone();
    // A3: destroyed is terminal. A8: the VersionState stays in metadata; only
    // the payload goes.
    find_version_mut(&mut new_meta.versions, version)?.destroyed = true;
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
    // Persist the new limit and stop: updating metadata does not prune
    // existing versions. The next write catches up, which is why `apply_put`'s
    // trim is a loop.
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

/// Build a storage key for metadata that is injective over all paths.
///
/// A9: length-prefix encoding, so a `:` inside `path` cannot shift the field
/// boundaries. Format: `m:{path_byte_len_hex8}:{path}`.
///
/// Readers must use [`parse_meta_key`] rather than a fixed byte offset.
///
/// # Example
/// ```
/// use kallisto_kv_model::apply::build_meta_key;
/// assert_eq!(build_meta_key("app/db"), "m:00000006:app/db");
/// ```
#[must_use]
pub fn build_meta_key(path: &str) -> String {
    format!("m:{:08x}:{}", path.len(), path)
}

/// Recover the path from a key produced by [`build_meta_key`].
///
/// Returns `None` for anything that is not a well-formed metadata key,
/// including keys in the pre-A9 `m:{path}` format.
///
/// # Example
/// ```
/// use kallisto_kv_model::apply::{build_meta_key, parse_meta_key};
/// assert_eq!(parse_meta_key(&build_meta_key("a:b")), Some("a:b"));
/// assert_eq!(parse_meta_key("m:app/db"), None); // pre-A9 format
/// ```
#[must_use]
pub fn parse_meta_key(key: &str) -> Option<&str> {
    let rest = key.strip_prefix("m:")?;
    let (len_hex, path) = rest.split_at_checked(8)?;
    let path = path.strip_prefix(':')?;
    let len = usize::from_str_radix(len_hex, 16).ok()?;
    // The declared length is what makes the encoding injective; a key whose body
    // does not match it is corrupt, not merely unusual.
    (path.len() == len).then_some(path)
}

/// Build a storage key for a versioned payload that is injective over all
/// `(path, version)` pairs.
///
/// A9: length-prefix encoding. Format:
/// `v:{path_byte_len_hex8}:{path}:{version}`.
///
/// # Example
/// ```
/// use kallisto_kv_model::apply::build_version_key;
/// assert_eq!(build_version_key("app/db", 3), "v:00000006:app/db:3");
/// // A colon in the path cannot forge another path's key:
/// assert_ne!(build_version_key("a:b", 1), build_version_key("a", 1));
/// ```
#[must_use]
pub fn build_version_key(path: &str, version: u32) -> String {
    format!("v:{:08x}:{}:{}", path.len(), path, version)
}
