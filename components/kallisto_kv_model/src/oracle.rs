//! Independent reference implementation of KV-v2 key state.
//!
//! This is the model oracle ADR-0013 V1 asks for, and it earns the name by
//! sharing no code with [`crate::apply`]. It keeps versions in a
//! `BTreeMap<u32, _>` keyed by version number rather than a `Vec`, and each
//! operation is written as a direct statement of the rule — an early return on
//! every skip — rather than in the factored form `apply` uses. Same contract,
//! deliberately different shape, so a mistake in one is unlikely to be mirrored
//! in the other.
//!
//! The differential property test in `crate::tests` runs random operation
//! sequences through both and requires identical observable state.
//!
//! An oracle that delegated to `apply` would be a tautology: it would pass no
//! matter what `apply` did. The previous version of this file did exactly that.

use std::collections::BTreeMap;

use crate::{
    apply::{DEFAULT_MAX_VERSIONS, KeyMetadata, VersionState},
    ops::KvOp,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OracleVersion {
    pub created_time_ms: u64,
    pub deletion_time_ms: u64,
    pub destroyed: bool,
}

/// Reference state for a single key.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Oracle {
    current_version: u32,
    oldest_version: u32,
    max_versions: u32,
    cas_required: bool,
    delete_version_after_ms: u64,
    versions: BTreeMap<u32, OracleVersion>,
}

/// Why the reference implementation refused an operation. Deliberately coarser
/// than `ModelError`: the differential test compares *acceptance*, so that a
/// mismatch in error taxonomy cannot masquerade as agreement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OracleReject {
    Cas,
}

impl Oracle {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Apply one operation. `Ok(())` covers both "mutated" and "skipped", which
    /// is what the HTTP layer reports to the caller either way.
    pub fn apply(&mut self, op: KvOp, now_ms: u64) -> Result<(), OracleReject> {
        match op {
            KvOp::Put { cas, .. } => self.put(cas, now_ms),
            KvOp::SoftDelete { version } => {
                self.soft_delete(version, now_ms);
                Ok(())
            }
            KvOp::Undelete { version } => {
                self.undelete(version, now_ms);
                Ok(())
            }
            KvOp::Destroy { version } => {
                self.destroy(version);
                Ok(())
            }
            KvOp::UpdateMeta {
                max_versions,
                cas_required,
                delete_version_after_ms,
            } => {
                // Assign and persist. No pruning here — the next write catches up.
                self.max_versions = max_versions;
                self.cas_required = cas_required;
                self.delete_version_after_ms = delete_version_after_ms;
                Ok(())
            }
        }
    }

    fn put(&mut self, cas: Option<u32>, now_ms: u64) -> Result<(), OracleReject> {
        match cas {
            Some(c) if c != self.current_version => return Err(OracleReject::Cas),
            None if self.cas_required => return Err(OracleReject::Cas),
            _ => {}
        }

        self.current_version += 1;
        let deletion_time_ms = if self.delete_version_after_ms == 0 {
            0
        } else {
            now_ms.saturating_add(self.delete_version_after_ms)
        };
        self.versions.insert(
            self.current_version,
            OracleVersion {
                created_time_ms: now_ms,
                deletion_time_ms,
                destroyed: false,
            },
        );

        let max = if self.max_versions == 0 {
            DEFAULT_MAX_VERSIONS
        } else {
            self.max_versions
        };
        if self.current_version - self.oldest_version >= max {
            let to_delete = self.current_version - max;
            let mut i = self.oldest_version;
            while i < to_delete + 1 {
                self.versions.remove(&i);
                i += 1;
            }
            self.oldest_version = to_delete + 1;
        }
        Ok(())
    }

    fn soft_delete(&mut self, version: u32, now_ms: u64) {
        let Some(lv) = self.versions.get_mut(&version) else {
            return; // no such version: skip
        };
        if lv.destroyed {
            return; // destroyed is terminal: skip
        }
        if lv.deletion_time_ms != 0 && lv.deletion_time_ms <= now_ms {
            return; // already past its deletion time: skip
        }
        lv.deletion_time_ms = now_ms;
    }

    fn undelete(&mut self, version: u32, now_ms: u64) {
        let dva = self.delete_version_after_ms;
        let Some(lv) = self.versions.get_mut(&version) else {
            return;
        };
        if lv.destroyed {
            return;
        }
        lv.deletion_time_ms = if dva == 0 {
            0
        } else {
            now_ms.saturating_add(dva)
        };
    }

    fn destroy(&mut self, version: u32) {
        let Some(lv) = self.versions.get_mut(&version) else {
            return;
        };
        if lv.destroyed {
            return;
        }
        lv.destroyed = true;
    }

    /// Project onto the shape `apply` produces, for comparison.
    #[must_use]
    pub fn to_key_metadata(&self) -> KeyMetadata {
        KeyMetadata {
            current_version: self.current_version,
            oldest_version: self.oldest_version,
            max_versions: self.max_versions,
            cas_required: self.cas_required,
            delete_version_after_ms: self.delete_version_after_ms,
            versions: self
                .versions
                .iter()
                .map(|(id, v)| VersionState {
                    created_time_ms: v.created_time_ms,
                    deletion_time_ms: v.deletion_time_ms,
                    version_id: *id,
                    destroyed: v.destroyed,
                })
                .collect(),
        }
    }
}
