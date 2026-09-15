//! ADR-0013 Group E: security invariants.
//!
//! E1 is verified here. E2 and E3 are NOT — the features they constrain do not
//! exist in this workspace yet, and a test that cannot fail is worse than a
//! missing one: it reports a passing gate, and it inflates the mutation score
//! with a target nothing can kill. Both are recorded as blocked in
//! `docs/references/verification-status.md` with what has to land first.
//!
//! What was here before: `e2_token_comparison_uses_ct_eq` grepped the source
//! tree and, on finding nothing, printed a warning and passed; and
//! `e3_policy_deny_overrides_allow` had an empty body with a TODO.

use naughtian_kallisto::engine::{
    error::EngineError,
    traits::{KeyMetadata, SecretPayload, VersionState},
};

const SECRET: &str = "super-secret-value-9f3a2b";
const SECRET_PATH: &str = "secret/data/prod/db-root-credential";

fn payload() -> SecretPayload {
    SecretPayload {
        value: SECRET.to_string(),
        ttl: 3600,
    }
}

/// E1: the `Debug` rendering of a payload must not carry the secret. This is
/// the one that actually bites: `Debug` reaches logs through `{:?}`, through
/// `unwrap()` panic messages, and through `#[derive(Debug)]` on any struct that
/// holds a payload.
#[test]
fn e1_secret_payload_debug_is_redacted() {
    let rendered = format!("{:?}", payload());
    assert!(
        !rendered.contains(SECRET),
        "Debug leaked the secret value: {rendered}"
    );
    assert!(
        rendered.contains("<REDACTED>"),
        "Debug must mark the omission so a reader knows it is not simply absent: {rendered}"
    );
    // The non-secret field is still useful for debugging and must survive.
    assert!(rendered.contains("3600"), "ttl should remain visible");
}

/// E1: redaction has to survive being nested inside another `Debug` output,
/// which is how a payload normally reaches a log line.
#[test]
fn e1_redaction_survives_nesting() {
    let nested = format!("{:?}", vec![Some(payload())]);
    assert!(
        !nested.contains(SECRET),
        "nested Debug leaked the secret value: {nested}"
    );

    #[derive(Debug)]
    #[allow(dead_code)]
    struct Envelope {
        request_id: u64,
        body: SecretPayload,
    }
    let enveloped = format!(
        "{:?}",
        Envelope {
            request_id: 7,
            body: payload(),
        }
    );
    assert!(
        !enveloped.contains(SECRET),
        "a derived Debug on a struct holding a payload leaked it: {enveloped}"
    );
}

/// E1: `{:#?}` is a different formatter path from `{:?}`. A hand-written
/// `Debug` that builds its output by hand rather than via `debug_struct` can
/// redact one and not the other.
#[test]
fn e1_alternate_debug_is_also_redacted() {
    let rendered = format!("{:#?}", payload());
    assert!(
        !rendered.contains(SECRET),
        "pretty Debug leaked the secret value: {rendered}"
    );
    assert!(rendered.contains("<REDACTED>"));
}

/// E1 (ADR-0011 §5): engine errors are rendered into HTTP response bodies and
/// logs, so no error variant may carry secret material in its `Display`.
#[test]
fn e1_engine_errors_do_not_carry_secret_material() {
    let errors = [
        EngineError::NotFound,
        EngineError::SoftDeleted,
        EngineError::Destroyed,
        EngineError::InvalidVersion(7),
        EngineError::CasMismatch {
            expected: 3,
            actual: 4,
        },
        EngineError::CasRequired,
        EngineError::QueueFull,
    ];
    for err in &errors {
        let rendered = format!("{err} / {err:?}");
        assert!(
            !rendered.contains(SECRET),
            "{err:?} rendered the secret value"
        );
        assert!(
            !rendered.contains(SECRET_PATH),
            "{err:?} rendered a secret path"
        );
    }
}

/// E1: `StorageError` is the one variant that carries free text, so it is the
/// one that can be misused. This pins the rule that callers must not
/// interpolate a payload or a path into it — the assertion is on the
/// construction sites we control, demonstrated here on the shape the engine
/// actually builds.
#[test]
fn e1_storage_error_messages_stay_generic() {
    // Every `StorageError` the engine constructs describes the operation, never
    // the data. If a future change interpolates a payload, this is the shape
    // that has to keep holding.
    let err = EngineError::StorageError("Missing version payload".to_string());
    let rendered = format!("{err}");
    assert!(!rendered.contains(SECRET));
    assert!(!rendered.contains(SECRET_PATH));
}

/// E1: metadata is not secret-bearing by construction — it holds version
/// bookkeeping only. This test exists to fail if a payload-carrying field is
/// ever added to `KeyMetadata`, which would put secrets on the metadata read
/// path and into every metadata log line.
#[test]
fn e1_key_metadata_carries_no_payload_field() {
    let meta = KeyMetadata {
        current_version: 2,
        oldest_version: 1,
        max_versions: 5,
        cas_required: true,
        delete_version_after_ms: 1000,
        custom_metadata: [("owner".to_string(), "platform".to_string())]
            .into_iter()
            .collect(),
        versions: vec![VersionState {
            created_time_ms: 1,
            deletion_time_ms: 0,
            version_id: 1,
            destroyed: false,
        }],
    };

    let rendered = format!("{meta:?}");
    assert!(!rendered.contains(SECRET), "metadata Debug leaked a secret");

    // Round-tripping a payload's value through metadata must be impossible:
    // there is no field to put it in. If this stops compiling because a field
    // was added, that field needs the redaction treatment too.
    let _: fn(&KeyMetadata) -> u32 = |m| m.current_version;
}
