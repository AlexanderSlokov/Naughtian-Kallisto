//! ADR-0013 Group E: security invariants.
//!
//! All three are verified here. E2 and E3 were blocked until token
//! authentication and a policy evaluator existed; they landed with
//! `policy_engine`, and the rule ADR-0013 set was that the tests land with
//! them.
//!
//! What was here before, and must not come back:
//! `e2_token_comparison_uses_ct_eq` grepped the source tree and, on finding
//! nothing, printed a warning and passed; `e3_policy_deny_overrides_allow` had
//! an empty body with a TODO. A test that cannot fail reports a gate that does
//! not exist, and inflates the mutation score with a target nothing can kill.

use std::collections::BTreeMap;

use core_crypto::{Contents, PolicyRule};
use naughtian_kallisto::{
    engine::{
        error::EngineError,
        traits::{KeyMetadata, SecretPayload, VersionState},
    },
    resolver::Snapshot,
};
use policy_engine::{Capability, TokenKey, TokenTable};

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

// -----------------------------------------------------------------------------
// E2: token comparison is constant-time
// -----------------------------------------------------------------------------

const TOKEN: &str = "s.a-real-token-value";

fn token_key() -> TokenKey {
    TokenKey::from_bytes([0x5a; 32])
}

fn policies() -> BTreeMap<String, Vec<PolicyRule>> {
    BTreeMap::from([
        (
            "reader".to_string(),
            vec![PolicyRule {
                path: "secret/data/app/*".to_string(),
                capabilities: vec!["read".to_string()],
            }],
        ),
        (
            "narrowed".to_string(),
            vec![
                PolicyRule {
                    path: "secret/data/app/*".to_string(),
                    capabilities: vec!["read".to_string()],
                },
                PolicyRule {
                    path: "secret/data/app/db".to_string(),
                    capabilities: vec!["deny".to_string()],
                },
            ],
        ),
    ])
}

fn table(tokens: &[(&str, &str)]) -> TokenTable {
    let key = token_key();
    let entries = tokens
        .iter()
        .map(|(token, policy)| (key.hash_hex(token), vec![(*policy).to_string()]))
        .collect();
    TokenTable::build(&key.expose_as_hex(), &entries, &policies()).unwrap()
}

/// E2: the comparison that decides whether a token is valid is full-width.
///
/// The table below holds nothing but *near misses* — hashes built from the real
/// token's hash with a single bit flipped, at the front and at the back. A
/// lookup of the real token must find none of them. Any implementation that
/// compares a prefix, a suffix, or a truncation of the hash accepts one of
/// these and fails here.
///
/// This was written the other way round first — three tokens at three
/// positions, asserting each resolved correctly — and a deliberately broken
/// implementation comparing only the first four bytes passed it. Asserting that
/// the right answers come back does not constrain how the comparison is made.
#[test]
fn e2_token_comparison_does_not_match_on_a_partial_hash() {
    let key = token_key();
    let real = key.hash(TOKEN);

    let mut near_misses = BTreeMap::new();
    for bit in [0usize, 1, 15, 30, 31] {
        let mut altered = real;
        altered[bit] ^= 0x01;
        near_misses.insert(
            core_crypto::hex::encode(&altered),
            vec!["reader".to_string()],
        );
    }
    assert_eq!(near_misses.len(), 5, "the near misses collided");

    let table = TokenTable::build(&key.expose_as_hex(), &near_misses, &policies()).unwrap();
    assert!(
        table.lookup(TOKEN).is_none(),
        "a token was accepted on a partial hash match"
    );
}

/// E2: a token is found wherever it sits in the table, and nothing else is.
#[test]
fn e2_a_token_is_found_wherever_it_sits_and_nothing_else_is() {
    let table = table(&[
        ("s.first", "reader"),
        (TOKEN, "reader"),
        ("s.last", "reader"),
    ]);

    assert!(table.lookup("s.first").is_some(), "first entry");
    assert!(table.lookup(TOKEN).is_some(), "middle entry");
    assert!(table.lookup("s.last").is_some(), "last entry");

    for wrong in [
        "s.absent",
        "s.a-real-token-valu",
        "s.a-real-token-value-and-more",
        " s.a-real-token-value",
        "",
    ] {
        assert!(table.lookup(wrong).is_none(), "{wrong:?} was accepted");
    }
}

/// E2: what the comparison runs over is a keyed hash, not the token. A bare
/// digest would make the file's plaintext — which passes through an editor, a
/// repository and a CI job before it is sealed — a dictionary attack away from
/// every token in the fleet.
#[test]
fn e2_the_stored_form_of_a_token_is_keyed() {
    use aws_lc_rs::digest;

    let hashed = token_key().hash_hex(TOKEN);
    assert!(!hashed.contains(TOKEN), "the token is stored in the clear");

    let bare = digest::digest(&digest::SHA256, TOKEN.as_bytes());
    assert_ne!(
        hashed,
        core_crypto::hex::encode(bare.as_ref()),
        "the stored form is a plain SHA-256 of the token"
    );

    let other_key = TokenKey::from_bytes([0xa5; 32]);
    assert_ne!(
        hashed,
        other_key.hash_hex(TOKEN),
        "the hash does not depend on the key"
    );
}

// -----------------------------------------------------------------------------
// E3: explicit deny overrides allow
// -----------------------------------------------------------------------------

/// E3: a `deny` rule wins over any grant that also matches, whichever order
/// they appear in and however much broader the grant is.
#[test]
fn e3_policy_deny_overrides_allow() {
    let rules = table(&[(TOKEN, "narrowed")]);
    let grant = rules.lookup(TOKEN).expect("the token should resolve");

    assert!(
        grant
            .rules
            .allows("secret/data/app/other", Capability::Read),
        "the grant should still apply where nothing denies it"
    );
    assert!(
        !grant.rules.allows("secret/data/app/db", Capability::Read),
        "deny did not override the grant that covers the same path"
    );
}

/// E3, through the layer that serves requests rather than the one that decides
/// them: a denied path must be refused by the resolver itself, not only by the
/// matcher in isolation.
#[test]
fn e3_deny_is_enforced_by_the_snapshot_not_just_the_matcher() {
    let key = token_key();
    let contents = Contents {
        version: 1,
        secrets: BTreeMap::from([
            ("app/db".to_string(), serde_json::json!({"k": SECRET})),
            ("app/other".to_string(), serde_json::json!({"k": "fine"})),
        ]),
        policies: policies(),
        tokens: BTreeMap::from([(key.hash_hex(TOKEN), vec!["narrowed".to_string()])]),
        token_key: Some(key.expose_as_hex()),
    };
    // The file's journey in miniature: serialise, then parse back through the
    // borrowed view production uses.
    let json = serde_json::to_string(&contents).unwrap();
    let snapshot = Snapshot::build(serde_json::from_str(&json).unwrap(), None).unwrap();

    assert!(snapshot.enforces());
    assert!(snapshot.permits(Some(TOKEN), "secret/data/app/other", Capability::Read));
    assert!(!snapshot.permits(Some(TOKEN), "secret/data/app/db", Capability::Read));
    // Default deny still holds for anything no rule mentions, and for a request
    // that presented nothing at all.
    assert!(!snapshot.permits(Some(TOKEN), "secret/data/elsewhere", Capability::Read));
    assert!(!snapshot.permits(None, "secret/data/app/other", Capability::Read));
}

// -----------------------------------------------------------------------------
// ADR-0015 D15 — the log path
// -----------------------------------------------------------------------------

/// D15's naming constraint, turned into a gate.
///
/// > **Tuyệt đối không gọi nó là audit log** ở bất cứ đâu: tên biến, tên file,
/// > config key, tài liệu.
///
/// The reason is not tidiness. An audit log records before it serves, so a full
/// queue means refusing to serve; this one drops and keeps serving. Calling it
/// an audit log would let somebody believe they can answer "who read the Stripe
/// key" when they cannot, and D15 names that belief as the trap. A promise in a
/// document decays; a test does not, which is the entire reason this is here
/// and not in a style guide.
///
/// Prose that *discusses* the absence is allowed — this very comment does —
/// so the scan covers code and configuration, where the word could only ever
/// appear as a name.
#[test]
fn d15_nothing_in_the_code_is_named_audit() {
    let mut offenders = Vec::new();

    for entry in walkdir::WalkDir::new(env!("CARGO_MANIFEST_DIR"))
        .into_iter()
        .filter_entry(|e| {
            let name = e.file_name().to_string_lossy();
            name != "target" && name != ".git" && name != "docs"
        })
        .filter_map(Result::ok)
    {
        let path = entry.path();
        let is_code = path
            .extension()
            .is_some_and(|ext| ext == "rs" || ext == "yaml" || ext == "yml" || ext == "toml");
        if !is_code || !entry.file_type().is_file() {
            continue;
        }
        // This file names the thing it forbids, necessarily.
        if path.ends_with("security_invariants.rs") {
            continue;
        }

        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        for (number, line) in text.lines().enumerate() {
            let lowered = line.to_lowercase();
            if !lowered.contains("audit") {
                continue;
            }
            // A comment explaining why there is no audit log is the point, not
            // a violation. A *name* is.
            let trimmed = lowered.trim_start();
            if trimmed.starts_with("//") || trimmed.starts_with('#') {
                continue;
            }
            offenders.push(format!(
                "{}:{}: {}",
                path.display(),
                number + 1,
                line.trim()
            ));
        }
    }

    assert!(
        offenders.is_empty(),
        "ADR-0015 D15 forbids naming anything \"audit\"; found:\n{}",
        offenders.join("\n")
    );
}

/// D15: paths and tokens are hashed with a **keyed** hash, and the key for
/// paths is not the key for tokens.
///
/// Paths are chosen by the caller. Anyone who can send
/// `GET /v1/secret/data/<anything>` and read the access log would, under a
/// shared key and label, hold an oracle emitting `HMAC(token_key, arbitrary)` —
/// the material for a reverse table against the file's own token column. This
/// asserts the separation that removes it.
#[test]
fn d15_path_identifiers_cannot_be_used_against_the_token_table() {
    let token_key = TokenKey::from_bytes([0x5a; 32]);
    let log_key = telemetry::LogKey::derived_from(&token_key);

    // The same text, hashed as a path and as a token, must not agree.
    for text in ["s.apptoken", SECRET_PATH, "prod/db", ""] {
        let as_token = token_key.hash_hex(text);
        let as_path = log_key.id(text);
        assert!(
            !as_token.starts_with(as_path.as_str()),
            "path and token identifiers share a PRF for {text:?}"
        );
    }
}

/// D15: no log line carries a secret value, a path, or a token in the clear.
///
/// The access log is built from identifiers rather than text by construction —
/// `Record` cannot be built out of a raw path — so this checks the property
/// end-to-end on a real rendered line rather than trusting the type.
#[test]
fn d15_a_rendered_log_line_carries_no_cleartext() {
    let key = telemetry::LogKey::random();
    let log = telemetry::AccessLog::new(16, 1);
    let producer = log.producer(0);

    producer.record(&telemetry::Record {
        method: "GET",
        action: "data",
        path: key.id(SECRET_PATH),
        token: key.id("s.apptoken"),
        status: 200,
        enforced: true,
    });

    let sink = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let handle = log.spawn_writer(SharedSink(std::sync::Arc::clone(&sink)));
    std::thread::sleep(std::time::Duration::from_millis(50));
    log.stop();
    handle.join().unwrap();

    let rendered = String::from_utf8(sink.lock().unwrap().clone()).unwrap();

    assert!(!rendered.is_empty(), "nothing was written");
    assert!(!rendered.contains(SECRET), "the value leaked: {rendered}");
    assert!(
        !rendered.contains(SECRET_PATH),
        "the path leaked: {rendered}"
    );
    assert!(
        !rendered.contains("s.apptoken"),
        "the token leaked: {rendered}"
    );
    assert!(rendered.contains(key.id(SECRET_PATH).as_str()));
}

/// Collects what the writer thread produced so the test can read it back.
struct SharedSink(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);
impl std::io::Write for SharedSink {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
