//! ADR-0013 Group E: security invariants, plus ADR-0015 D15's log constraints.
//!
//! All three of Group E are verified here. E2 and E3 were blocked until token
//! authentication and a policy evaluator existed; they landed with
//! `policy_engine`, and the rule ADR-0013 set was that the tests land with
//! them.
//!
//! What was here before, and must not come back:
//! `e2_token_comparison_uses_ct_eq` grepped the source tree and, on finding
//! nothing, printed a warning and passed; `e3_policy_deny_overrides_allow` had
//! an empty body with a TODO. A test that cannot fail reports a gate that does
//! not exist, and inflates the mutation score with a target nothing can kill.
//!
//! E1 was rewritten when the storage engine was deleted. It used to be checked
//! against `SecretPayload`, `KeyMetadata` and `EngineError` — types belonging
//! to the write path that no longer exists. The invariant did not change; its
//! subjects did, and they are now the types a secret actually passes through:
//! the sealed file's `Contents`, the live `Snapshot`, every key, and every
//! error that can reach a log line.

use std::collections::BTreeMap;

use core_crypto::{Contents, PolicyRule, SealError, SealKey};
use naughtian_kallisto::{config::ConfigError, resolver::Snapshot};
use policy_engine::{Capability, TokenError, TokenKey, TokenTable};

const SECRET: &str = "super-secret-value-9f3a2b";
const SECRET_PATH: &str = "prod/db-root-credential";

fn contents() -> Contents {
    Contents {
        version: 3,
        secrets: BTreeMap::from([(
            SECRET_PATH.to_string(),
            serde_json::json!({ "password": SECRET }),
        )]),
        policies: BTreeMap::from([(
            "db".to_string(),
            vec![PolicyRule {
                path: "secret/data/prod/*".to_string(),
                capabilities: vec!["read".to_string()],
            }],
        )]),
        tokens: BTreeMap::new(),
        token_key: None,
    }
}

fn snapshot(contents: &Contents) -> Snapshot {
    let json = serde_json::to_string(contents).unwrap();
    let key = SealKey::from_bytes([4u8; 32]);
    let sealed =
        core_crypto::seal(&serde_json::from_str::<Contents>(&json).unwrap(), &key).unwrap();
    let opened = core_crypto::open(&sealed, &key, None).unwrap();
    Snapshot::build(opened.view().unwrap(), Some("\"etag\"".into())).unwrap()
}

/// E1: the `Debug` rendering of the decrypted file must not carry a secret.
///
/// This is the one that actually bites: `Debug` reaches logs through `{:?}`,
/// through `unwrap()` panic messages, and through `#[derive(Debug)]` on any
/// struct that happens to hold one of these.
#[test]
fn e1_the_decrypted_contents_debug_is_redacted() {
    let rendered = format!("{:?}", contents());
    assert!(
        !rendered.contains(SECRET),
        "Debug leaked the secret value: {rendered}"
    );
    assert!(
        !rendered.contains(SECRET_PATH),
        "Debug leaked a secret path: {rendered}"
    );
    assert!(
        rendered.contains("REDACTED"),
        "Debug must mark the omission so a reader knows it is not simply absent: {rendered}"
    );
    // The non-secret bookkeeping is still useful and must survive.
    assert!(rendered.contains('3'), "the version should remain visible");
}

/// E1: redaction has to survive being nested inside another `Debug` output,
/// which is how one of these normally reaches a log line.
#[test]
fn e1_redaction_survives_nesting() {
    let nested = format!("{:?}", vec![Some(contents())]);
    assert!(
        !nested.contains(SECRET),
        "nested Debug leaked the secret value: {nested}"
    );

    #[derive(Debug)]
    #[allow(dead_code)]
    struct Envelope {
        request_id: u64,
        body: Contents,
    }
    let enveloped = format!(
        "{:?}",
        Envelope {
            request_id: 7,
            body: contents(),
        }
    );
    assert!(
        !enveloped.contains(SECRET),
        "a derived Debug on a struct holding the contents leaked it: {enveloped}"
    );
}

/// E1: `{:#?}` is a different formatter path from `{:?}`. A hand-written
/// `Debug` that builds its output by hand rather than via `debug_struct` can
/// redact one and not the other.
#[test]
fn e1_alternate_debug_is_also_redacted() {
    let rendered = format!("{:#?}", contents());
    assert!(
        !rendered.contains(SECRET),
        "pretty Debug leaked the secret value: {rendered}"
    );
    assert!(rendered.contains("REDACTED"));

    let snapshot = snapshot(&contents());
    let rendered = format!("{snapshot:#?}");
    assert!(!rendered.contains(SECRET), "{rendered}");
    assert!(!rendered.contains(SECRET_PATH), "{rendered}");
}

/// E1 (ADR-0011 §5, ADR-0015 D15): every error that can reach a log or an HTTP
/// body, from every crate on the read path.
///
/// The list is deliberately exhaustive rather than representative. An error
/// type added later without redaction is exactly the kind of leak that is
/// invisible until the day it matters, and the only defence is that this list
/// has to be extended when one appears.
#[test]
fn e1_no_error_that_can_reach_a_log_carries_secret_material() {
    let sealed = core_crypto::seal(&contents(), &SealKey::from_bytes([1u8; 32])).unwrap();

    let mut rendered = Vec::new();
    let mut record = |what: String| rendered.push(what);

    for error in [
        SealError::BadMagic,
        SealError::AuthFailed,
        SealError::UnsupportedFormat { got: 9 },
        SealError::Rollback {
            held: 7,
            offered: 5,
        },
        SealError::RandomUnavailable,
    ] {
        record(format!("{error} / {error:?}"));
    }
    // The variants that can only be produced by actually trying it.
    for attempt in [
        core_crypto::open(b"", &SealKey::from_bytes([1u8; 32]), None),
        core_crypto::open(&sealed, &SealKey::from_bytes([2u8; 32]), None),
        core_crypto::open(&sealed, &SealKey::from_bytes([1u8; 32]), Some(99)),
    ] {
        let error = attempt.err().expect("these should all fail");
        record(format!("{error} / {error:?}"));
    }

    for error in [
        TokenError::KeyMissing { tokens: 2 },
        TokenError::HashMalformed { index: 1 },
    ] {
        record(format!("{error} / {error:?}"));
    }
    let error = core_crypto::hex::decode_into(SECRET, &mut [0u8; 32]).unwrap_err();
    record(format!("{error} / {error:?}"));

    for error in [
        ConfigError::NoWorkers,
        ConfigError::Missing,
        ConfigError::Kind {
            expected: "Resolver",
            got: "Wrong".to_string(),
        },
    ] {
        record(format!("{error} / {error:?}"));
    }

    // `ConfigError::Malformed` carries the parser's message whole, and an
    // unknown field names the *field* rather than its value — which is the
    // case an operator hits most and the one worth checking.
    let malformed = naughtian_kallisto::config::parse_file(
        &format!(
            "apiVersion: kallisto/v1\nkind: Resolver\nspec:\n  mount: {SECRET_PATH}\n  nope: {SECRET}\n"
        ),
        std::path::Path::new("bad.yaml"),
    )
    .unwrap_err();
    record(format!("{malformed}"));

    for text in &rendered {
        assert!(
            !text.contains(SECRET),
            "an error rendered the secret: {text}"
        );
        assert!(
            !text.contains(SECRET_PATH),
            "an error rendered a secret path: {text}"
        );
    }
}

/// E1's boundary, asserted rather than left to be discovered.
///
/// A *type* error from the configuration parser does echo the offending value:
/// "invalid type: string \"...\", expected usize". That is deliberate, and it
/// is the one place in this program where an error message quotes its input.
///
/// The reasoning: the configuration file is not secret-bearing by design
/// (ADR-0003 — the seal key and the bucket credentials have no field to go in,
/// and `deny_unknown_fields` means inventing one fails rather than being
/// ignored), while "line 4 is wrong" with no reason is a message that costs an
/// operator an hour. The sealed *secrets* file gets the opposite treatment:
/// `kallisto-ctl` withholds serde's message there precisely because the value
/// it choked on is a secret.
///
/// This test exists so that the trade is recorded and cannot be mistaken for an
/// oversight. If it ever becomes wrong — if a credential gains a config field —
/// this is the test that has to change first.
#[test]
fn e1_configuration_type_errors_quote_the_value_and_that_is_on_purpose() {
    let planted = "not-a-number-9f3a2b";
    let error = naughtian_kallisto::config::parse_file(
        &format!(
            "apiVersion: kallisto/v1\nkind: Resolver\nspec:\n  workers: {planted}\n  \
             source:\n    type: disk\n    path: /x\n"
        ),
        std::path::Path::new("bad.yaml"),
    )
    .unwrap_err();

    let rendered = error.to_string();
    assert!(
        rendered.contains(planted),
        "if this stops echoing the value the doc comment above is stale: {rendered}"
    );
    assert!(
        rendered.contains("line 4"),
        "the position is the part that makes the message worth the trade: {rendered}"
    );
}

/// E1: nothing that holds key material may render it.
///
/// Every one of these has a hand-written `Debug`, which is precisely the kind
/// of thing a later `#[derive(Debug)]` silently undoes.
#[test]
fn e1_key_material_never_renders_itself() {
    let token_key = TokenKey::from_bytes([0xab; 32]);
    let table = TokenTable::build(
        &token_key.expose_as_hex(),
        &BTreeMap::from([(token_key.hash_hex("s.token"), vec!["db".to_string()])]),
        &BTreeMap::new(),
    )
    .unwrap();

    let renderings = [
        format!("{:?}", SealKey::from_bytes([0xab; 32])),
        format!("{token_key:?}"),
        format!("{table:?}"),
        format!("{:?}", telemetry::LogKey::from_bytes([0xab; 32])),
    ];

    for rendered in &renderings {
        assert!(
            rendered.contains("REDACTED"),
            "key material rendered without marking the omission: {rendered}"
        );
        assert!(
            !rendered.contains("abab"),
            "key bytes leaked into Debug: {rendered}"
        );
        assert!(
            !rendered.contains("s.token"),
            "a token leaked into Debug: {rendered}"
        );
    }
}

/// E1: a live snapshot holds sealed bytes and nothing else readable.
///
/// The counterpart to the old `e1_key_metadata_carries_no_payload_field`: that
/// one asserted a struct had no field to put a secret in, this one asserts the
/// struct that *does* hold secrets never renders them and never stores them in
/// the clear (ADR-0015 D13).
#[test]
fn e1_a_live_snapshot_renders_counts_and_stores_ciphertext() {
    let snapshot = snapshot(&contents());

    let rendered = format!("{snapshot:?}");
    assert!(!rendered.contains(SECRET), "Snapshot Debug leaked a secret");
    assert!(
        !rendered.contains(SECRET_PATH),
        "Snapshot Debug leaked a path"
    );
    assert!(rendered.contains("REDACTED"));

    // And the bytes it holds are not the bytes it was given.
    for sealed in snapshot.sealed_secrets() {
        assert!(
            !sealed
                .ciphertext()
                .windows(SECRET.len())
                .any(|window| window == SECRET.as_bytes()),
            "a live snapshot holds the cleartext"
        );
    }
    // The secret is still servable — otherwise the assertion above would pass
    // for a snapshot that simply lost it.
    assert!(
        snapshot
            .with_secret(SECRET_PATH, ToString::to_string)
            .unwrap()
            .unwrap()
            .contains(SECRET)
    );
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
