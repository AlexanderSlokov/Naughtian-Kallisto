//! ADR-0013 Group A property tests.
//!
//! Every test here is fail-able; `docs/references/verification-status.md`
//! records the exact mutation that breaks each one.

use proptest::prelude::*;

use crate::{
    apply::{
        DEFAULT_MAX_VERSIONS, KeyMetadata, ModelError, apply, build_meta_key, build_version_key,
        effective_max_versions, is_readable, parse_meta_key,
    },
    effects::Effect,
    ops::KvOp,
    oracle::Oracle,
};

// --- Strategies ---

fn arb_op() -> impl Strategy<Value = KvOp> {
    prop_oneof![
        // Weighted towards Put so sequences actually build up versions to trim.
        4 => (0usize..1024, proptest::option::of(0u32..12))
            .prop_map(|(payload_len, cas)| KvOp::Put { payload_len, cas }),
        2 => (1u32..15).prop_map(|version| KvOp::SoftDelete { version }),
        2 => (1u32..15).prop_map(|version| KvOp::Undelete { version }),
        2 => (1u32..15).prop_map(|version| KvOp::Destroy { version }),
        1 => (0u32..6, any::<bool>(), 0u64..5_000).prop_map(
            |(max_versions, cas_required, delete_version_after_ms)| KvOp::UpdateMeta {
                max_versions,
                cas_required,
                delete_version_after_ms,
            }
        ),
    ]
}

fn arb_op_sequence() -> impl Strategy<Value = Vec<KvOp>> {
    proptest::collection::vec(arb_op(), 1..40)
}

/// Clock that advances by 1ms per operation, starting at 1000.
fn tick(step: usize) -> u64 {
    1000 + step as u64
}

fn run(ops: &[KvOp]) -> KeyMetadata {
    let mut meta = KeyMetadata::default();
    for (step, op) in ops.iter().enumerate() {
        if let Ok((next, _)) = apply(&meta, op.clone(), tick(step)) {
            meta = next;
        }
    }
    meta
}

// --- Differential: apply vs. the independent oracle ---

proptest! {
    /// The heart of V1. `apply` and `oracle` share no code; after any operation
    /// sequence their observable state must be identical, and they must agree on
    /// which operations were refused.
    #[test]
    fn prop_matches_oracle(ops in arb_op_sequence()) {
        let mut meta = KeyMetadata::default();
        let mut oracle = Oracle::new();

        for (step, op) in ops.iter().enumerate() {
            let now = tick(step);
            let model = apply(&meta, op.clone(), now);
            let reference = oracle.apply(op.clone(), now);

            prop_assert_eq!(
                model.is_ok(),
                reference.is_ok(),
                "step {}: apply returned {:?} but the oracle returned {:?} for {:?}",
                step, model.as_ref().map(|_| ()), reference, op
            );

            if let Ok((next, _)) = model {
                meta = next;
            }
            prop_assert_eq!(
                &meta,
                &oracle.to_key_metadata(),
                "step {}: state diverged after {:?}",
                step, op
            );
        }
    }
}

// --- A1: current_version monotonically increasing ---

proptest! {
    #[test]
    fn prop_a1_current_version_monotone(ops in arb_op_sequence()) {
        let mut meta = KeyMetadata::default();
        let mut previous = meta.current_version;
        for (step, op) in ops.iter().enumerate() {
            if let Ok((next, _)) = apply(&meta, op.clone(), tick(step)) {
                prop_assert!(
                    next.current_version >= previous,
                    "current_version went {} -> {} on {:?}",
                    previous, next.current_version, op
                );
                previous = next.current_version;
                meta = next;
            }
        }
    }
}

// --- A2: versions ordered ascending, no duplicates ---

proptest! {
    #[test]
    fn prop_a2_versions_strictly_ascending(ops in arb_op_sequence()) {
        let meta = run(&ops);
        for pair in meta.versions.windows(2) {
            prop_assert!(
                pair[0].version_id < pair[1].version_id,
                "versions not strictly ascending: {} then {}",
                pair[0].version_id, pair[1].version_id
            );
        }
    }
}

// --- A3: destroyed = true is terminal ---

proptest! {
    #[test]
    fn prop_a3_destroyed_is_terminal(ops in arb_op_sequence()) {
        let mut meta = KeyMetadata::default();
        let mut ever_destroyed = std::collections::HashSet::new();

        for (step, op) in ops.iter().enumerate() {
            if let Ok((next, _)) = apply(&meta, op.clone(), tick(step)) {
                for vs in &next.versions {
                    if vs.destroyed {
                        ever_destroyed.insert(vs.version_id);
                    }
                }
                for id in &ever_destroyed {
                    // Trimmed out entirely is fine: the version is gone, not
                    // resurrected. Present-but-not-destroyed is the violation.
                    if let Some(vs) = next.versions.iter().find(|v| v.version_id == *id) {
                        prop_assert!(
                            vs.destroyed,
                            "version {} came back un-destroyed after {:?}",
                            id, op
                        );
                    }
                }
                meta = next;
            }
        }
    }
}

// --- A4: CAS mismatch leaves state untouched ---

proptest! {
    #[test]
    fn prop_a4_cas_mismatch_is_inert(ops in arb_op_sequence(), wrong in 0u32..60) {
        let meta = run(&ops);
        let cas = if wrong == meta.current_version { wrong.wrapping_add(1) } else { wrong };

        let before = meta.clone();
        let result = apply(&meta, KvOp::Put { payload_len: 10, cas: Some(cas) }, 9_000);

        prop_assert_eq!(
            result.unwrap_err(),
            ModelError::CasMismatch { expected: cas, actual: before.current_version }
        );
        prop_assert_eq!(&meta, &before, "CAS mismatch must not mutate state");
    }
}

proptest! {
    /// `cas_required` must reject a write that carries no `cas` at all
    /// This is what the previous model ignored outright.
    #[test]
    fn prop_a4_cas_required_rejects_missing_cas(n_puts in 0u32..4) {
        let mut meta = KeyMetadata { cas_required: true, ..KeyMetadata::default() };
        for i in 0..n_puts {
            let (next, _) = apply(
                &meta,
                KvOp::Put { payload_len: 8, cas: Some(meta.current_version) },
                tick(i as usize),
            ).unwrap();
            meta = next;
        }

        let before = meta.clone();
        prop_assert_eq!(
            apply(&meta, KvOp::Put { payload_len: 8, cas: None }, 9_000).unwrap_err(),
            ModelError::CasRequired
        );
        prop_assert_eq!(&meta, &before);

        // ...and must accept the matching cas.
        let (next, _) = apply(
            &meta,
            KvOp::Put { payload_len: 8, cas: Some(meta.current_version) },
            9_001,
        ).unwrap();
        prop_assert_eq!(next.current_version, before.current_version + 1);
    }
}

// --- A5: a written version is readable at the version the write reported ---

proptest! {
    #[test]
    fn prop_a5_put_yields_a_readable_version(payload_len in 0usize..4096, dva in 0u64..1000) {
        let meta = KeyMetadata { delete_version_after_ms: dva, ..KeyMetadata::default() };
        let (next, effects) = apply(
            &meta,
            KvOp::Put { payload_len, cas: None },
            5_000,
        ).unwrap();

        prop_assert_eq!(next.current_version, 1);
        let vs = next.versions.iter().find(|v| v.version_id == 1).unwrap();
        prop_assert!(
            is_readable(vs, 5_000),
            "the version a successful put created is not readable at write time"
        );
        prop_assert!(
            effects.contains(&Effect::WriteVersion { version: 1 }),
            "put must emit WriteVersion for the version it reported"
        );
        // The engine must be told to persist metadata and index the path too,
        // or a restart loses the key from LIST.
        prop_assert!(effects.contains(&Effect::WriteMeta));
        prop_assert!(effects.contains(&Effect::IndexPath));

        // With a TTL configured, the version stops being readable once it passes.
        if dva > 0 {
            prop_assert!(!is_readable(vs, 5_000 + dva));
        }
    }
}

// --- A6: undelete ∘ soft_delete = identity for non-destroyed versions ---

proptest! {
    #[test]
    fn prop_a6_undelete_inverts_soft_delete(n_puts in 1u32..5, target in 1u32..5) {
        // TTL disabled: with `delete_version_after` set the timer is re-armed
        // on undelete, so the inverse only holds in the untimed case. That
        // carve-out is verified by prop_a6_undelete_rearms_ttl below.
        let mut meta = KeyMetadata::default();
        for i in 0..n_puts {
            let (next, _) = apply(&meta, KvOp::Put { payload_len: 10, cas: None }, tick(i as usize))
                .unwrap();
            meta = next;
        }
        prop_assume!(meta.versions.iter().any(|v| v.version_id == target));

        let before = meta.clone();
        let (deleted, _) = apply(&meta, KvOp::SoftDelete { version: target }, 7_000).unwrap();
        prop_assert!(!is_readable(
            deleted.versions.iter().find(|v| v.version_id == target).unwrap(),
            7_000
        ));

        let (restored, _) = apply(&deleted, KvOp::Undelete { version: target }, 7_001).unwrap();
        prop_assert_eq!(&restored, &before, "undelete did not invert soft_delete");
    }
}

proptest! {
    #[test]
    fn prop_a6_undelete_rearms_ttl(dva in 1u64..1000) {
        let meta = KeyMetadata { delete_version_after_ms: dva, ..KeyMetadata::default() };
        let (meta, _) = apply(&meta, KvOp::Put { payload_len: 4, cas: None }, 1_000).unwrap();
        let (meta, _) = apply(&meta, KvOp::SoftDelete { version: 1 }, 1_500).unwrap();
        let (meta, _) = apply(&meta, KvOp::Undelete { version: 1 }, 2_000).unwrap();

        let vs = meta.versions.iter().find(|v| v.version_id == 1).unwrap();
        prop_assert_eq!(
            vs.deletion_time_ms, 2_000 + dva,
            "undelete must re-arm delete_version_after from the undelete time"
        );
        prop_assert!(is_readable(vs, 2_000));
    }
}

// --- A7: versions.len() <= max_versions after trimming ---

proptest! {
    /// A7, stated precisely. ADR-0013 words it as `versions.len() <=
    /// max_versions after trimming`, which is only true *after a write*:
    /// updating metadata lowers the limit without pruning, so a key can sit
    /// above its new limit until the next Put catches up (verified separately in
    /// `a7_lowering_the_limit_defers_to_the_next_write`).
    ///
    /// Destroy interleaved with Put is the case the previous model got wrong: it
    /// skipped destroyed versions when trimming, and so trimmed the version it
    /// had just written.
    #[test]
    fn prop_a7_max_versions_enforced_after_write(ops in arb_op_sequence()) {
        let mut meta = KeyMetadata::default();
        for (step, op) in ops.iter().enumerate() {
            if let Ok((next, effects)) = apply(&meta, op.clone(), tick(step)) {
                if matches!(op, KvOp::Put { .. }) {
                    let limit = effective_max_versions(&next);
                    prop_assert!(
                        next.versions.len() as u32 <= limit,
                        "versions.len()={} exceeds max_versions={} after {:?}",
                        next.versions.len(), limit, op
                    );
                    // The version a Put just created must survive that same Put.
                    let created = next.current_version;
                    prop_assert!(
                        next.versions.iter().any(|v| v.version_id == created),
                        "put reported version {} then trimmed it away; effects={:?}",
                        created, effects
                    );
                    prop_assert!(
                        !effects.contains(&Effect::TrimVersion { version: created }),
                        "put emitted TrimVersion for the version it just wrote"
                    );
                }
                meta = next;
            }
        }
    }
}

#[test]
fn a7_lowering_the_limit_defers_to_the_next_write() {
    // Updating metadata assigns the new limit and persists it; it does not
    // prune. The write path's catch-up loop does the pruning on the next write,
    // in one pass.
    let mut meta = KeyMetadata::default();
    for i in 0..3 {
        let (next, _) = apply(
            &meta,
            KvOp::Put {
                payload_len: 1,
                cas: None,
            },
            tick(i),
        )
        .unwrap();
        meta = next;
    }
    assert_eq!(meta.versions.len(), 3);

    let (meta, _) = apply(
        &meta,
        KvOp::UpdateMeta {
            max_versions: 1,
            cas_required: false,
            delete_version_after_ms: 0,
        },
        tick(3),
    )
    .unwrap();
    assert_eq!(
        meta.versions.len(),
        3,
        "lowering max_versions must not retroactively prune"
    );

    let (meta, effects) = apply(
        &meta,
        KvOp::Put {
            payload_len: 1,
            cas: None,
        },
        tick(4),
    )
    .unwrap();
    assert_eq!(
        meta.versions.len(),
        1,
        "the next write must catch up in one pass"
    );
    assert_eq!(meta.current_version, 4);
    assert_eq!(meta.oldest_version, 4);
    for victim in 1..=3 {
        assert!(
            effects.contains(&Effect::TrimVersion { version: victim }),
            "version {victim} was dropped from metadata without a TrimVersion effect, \
             so its payload would be orphaned in storage"
        );
    }
}

proptest! {
    /// Trimming counts destroyed versions rather than skipping them, so the
    /// retained window is always the newest `max_versions` version numbers.
    #[test]
    fn prop_a7_retains_newest_window(max_v in 1u32..6, n_puts in 1u32..20) {
        let mut meta = KeyMetadata { max_versions: max_v, ..KeyMetadata::default() };
        for i in 0..n_puts {
            let (next, _) = apply(&meta, KvOp::Put { payload_len: 1, cas: None }, tick(i as usize))
                .unwrap();
            meta = next;
            // Destroy the oldest surviving version to prove it still counts.
            if let Some(oldest) = meta.versions.first().map(|v| v.version_id) {
                let (next, _) = apply(&meta, KvOp::Destroy { version: oldest }, tick(i as usize))
                    .unwrap();
                meta = next;
            }
        }

        let lowest = meta.current_version.saturating_sub(max_v - 1);
        for vs in &meta.versions {
            prop_assert!(
                vs.version_id >= lowest,
                "version {} survived below the retained window [{}, {}]",
                vs.version_id, lowest, meta.current_version
            );
        }
        prop_assert!(meta.versions.iter().any(|v| v.version_id == meta.current_version));
    }
}

#[test]
fn a7_default_limit_applies_when_unset() {
    // max_versions = 0 means "use the default", not "keep everything".
    let mut meta = KeyMetadata::default();
    for i in 0..(DEFAULT_MAX_VERSIONS + 5) {
        let (next, _) = apply(
            &meta,
            KvOp::Put {
                payload_len: 1,
                cas: None,
            },
            tick(i as usize),
        )
        .unwrap();
        meta = next;
    }
    assert_eq!(meta.versions.len() as u32, DEFAULT_MAX_VERSIONS);
    assert_eq!(meta.current_version, DEFAULT_MAX_VERSIONS + 5);
}

// --- A8: destroy removes the payload, keeps the VersionState ---

#[test]
fn a8_destroy_retains_version_state() {
    let meta = KeyMetadata::default();
    let (meta, _) = apply(
        &meta,
        KvOp::Put {
            payload_len: 10,
            cas: None,
        },
        1_000,
    )
    .unwrap();
    let (destroyed, effects) = apply(&meta, KvOp::Destroy { version: 1 }, 2_000).unwrap();

    let vs = destroyed
        .versions
        .iter()
        .find(|v| v.version_id == 1)
        .expect("VersionState must be retained after destroy");
    assert!(vs.destroyed);
    assert!(!is_readable(vs, 2_000));
    assert!(effects.contains(&Effect::DeleteVersion { version: 1 }));

    // Re-destroying is a no-op, not a second payload delete.
    let (again, effects) = apply(&destroyed, KvOp::Destroy { version: 1 }, 2_001).unwrap();
    assert_eq!(again, destroyed);
    assert!(effects.is_empty());
}

#[test]
fn a3_undelete_on_destroyed_is_a_noop() {
    let meta = KeyMetadata::default();
    let (meta, _) = apply(
        &meta,
        KvOp::Put {
            payload_len: 4,
            cas: None,
        },
        1_000,
    )
    .unwrap();
    let (meta, _) = apply(&meta, KvOp::Destroy { version: 1 }, 1_100).unwrap();

    // A destroyed version is skipped, not rejected.
    let (after, effects) = apply(&meta, KvOp::Undelete { version: 1 }, 1_200).unwrap();
    assert_eq!(
        after, meta,
        "undelete must not resurrect a destroyed version"
    );
    assert!(effects.is_empty());

    let (after, effects) = apply(&meta, KvOp::SoftDelete { version: 1 }, 1_300).unwrap();
    assert_eq!(
        after, meta,
        "soft_delete must not touch a destroyed version"
    );
    assert!(effects.is_empty());
}

// --- A9: (path, version) -> storage key is injective ---

proptest! {
    #[test]
    fn prop_a9_storage_keys_are_injective(
        path_a in "[a-z:./]{0,20}",
        path_b in "[a-z:./]{0,20}",
        ver_a in 0u32..100,
        ver_b in 0u32..100,
    ) {
        if (path_a.as_str(), ver_a) != (path_b.as_str(), ver_b) {
            prop_assert_ne!(
                build_version_key(&path_a, ver_a),
                build_version_key(&path_b, ver_b)
            );
        }
        if path_a != path_b {
            prop_assert_ne!(build_meta_key(&path_a), build_meta_key(&path_b));
        }
    }
}

proptest! {
    /// Injectivity is worth nothing if the reader cannot invert the encoding.
    /// The path index rebuild used a fixed `key[2..]` offset and silently
    /// produced `00000006:app/db` as a secret path after the format change.
    #[test]
    fn prop_a9_meta_key_roundtrips(path in "[ -~]{0,40}") {
        let key = build_meta_key(&path);
        prop_assert_eq!(parse_meta_key(&key), Some(path.as_str()));
    }
}

#[test]
fn a9_rejects_pre_length_prefix_keys() {
    // The old format must not be mistaken for a path: a stale key has to be
    // visibly unreadable rather than yielding a wrong path.
    assert_eq!(parse_meta_key("m:app/db"), None);
    assert_eq!(parse_meta_key("v:00000006:app/db"), None);
    assert_eq!(parse_meta_key("m:0000ffff:short"), None);
}

#[test]
fn a9_key_encoding_format_is_stable() {
    assert_eq!(build_meta_key("app/db"), "m:00000006:app/db");
    assert_eq!(build_version_key("app/db", 3), "v:00000006:app/db:3");
    // The pre-A9 collision: `v:{path}:{version}` made these two identical.
    assert_ne!(build_version_key("a:1", 2), build_version_key("a", 1));
}
