//! ADR-0013 V3 / C1: rkyv serialisation round-trip under adversarial *values*.
//!
//! Scope, stated plainly. The engine reads storage bytes with
//! `unsafe { rkyv::archived_root::<T>(bytes) }`, which has no validation: for
//! arbitrary attacker-chosen bytes it is undefined behaviour *by contract*, so a
//! target that fed raw fuzz bytes straight into it would report "crashes" that
//! say nothing about Kallisto — an unaligned slice alone is enough. A
//! permanently-red target is worse than no target (ADR-0013 §5).
//!
//! So this target fuzzes the path that actually has a contract to keep: build an
//! arbitrary *well-formed* value, serialise it the way the engine does, read it
//! back through `archived_root`, and require the value to survive intact. That
//! covers serialiser arithmetic, the archived layout, and every string/vec/map
//! length the fuzzer can invent.
//!
//! Corrupted-bytes resistance is NOT covered here and is recorded as an
//! unproven invariant in `docs/references/verification-status.md`: closing it
//! needs `#[archive(check_bytes)]` plus a validated read path, which is the same
//! work as the rkyv 0.8 migration.

#![no_main]

use std::collections::HashMap;

use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;
use naughtian_kallisto::engine::traits::{KeyMetadata, SecretPayload, VersionState};
use rkyv::{Deserialize, archived_root, ser::Serializer, ser::serializers::AllocSerializer};

#[derive(Arbitrary, Debug)]
struct ArbVersion {
    created_time_ms: u64,
    deletion_time_ms: u64,
    version_id: u32,
    destroyed: bool,
}

#[derive(Arbitrary, Debug)]
struct ArbInput {
    current_version: u32,
    oldest_version: u32,
    max_versions: u32,
    cas_required: bool,
    delete_version_after_ms: u64,
    custom_metadata: Vec<(String, String)>,
    versions: Vec<ArbVersion>,
    payload_value: String,
    payload_ttl: u64,
}

fuzz_target!(|input: ArbInput| {
    let meta = KeyMetadata {
        current_version: input.current_version,
        oldest_version: input.oldest_version,
        max_versions: input.max_versions,
        cas_required: input.cas_required,
        delete_version_after_ms: input.delete_version_after_ms,
        custom_metadata: input.custom_metadata.into_iter().collect::<HashMap<_, _>>(),
        versions: input
            .versions
            .into_iter()
            .map(|v| VersionState {
                created_time_ms: v.created_time_ms,
                deletion_time_ms: v.deletion_time_ms,
                version_id: v.version_id,
                destroyed: v.destroyed,
            })
            .collect(),
    };

    // Mirrors KvEngine::serialize_metadata.
    let mut ser = AllocSerializer::<1024>::default();
    ser.serialize_value(&meta).expect("serialize metadata");
    let bytes = ser.into_serializer().into_inner();

    // SAFETY: `bytes` is an archive this target just produced with the same
    // rkyv version and type, and `AlignedVec` guarantees the alignment
    // `archived_root` requires. This is exactly the engine's read path.
    let archived = unsafe { archived_root::<KeyMetadata>(&bytes) };
    let restored: KeyMetadata = archived
        .deserialize(&mut rkyv::Infallible)
        .expect("deserialize metadata");
    assert_eq!(restored, meta, "KeyMetadata did not survive the round-trip");

    let payload = SecretPayload {
        value: input.payload_value,
        ttl: input.payload_ttl,
    };
    let mut ser = AllocSerializer::<1024>::default();
    ser.serialize_value(&payload).expect("serialize payload");
    let bytes = ser.into_serializer().into_inner();

    // SAFETY: as above.
    let archived = unsafe { archived_root::<SecretPayload>(&bytes) };
    assert_eq!(
        archived.value.as_str(),
        payload.value,
        "SecretPayload value did not survive the round-trip"
    );
    assert_eq!(archived.ttl, payload.ttl);
});
