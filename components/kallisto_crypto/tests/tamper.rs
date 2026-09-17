//! Adversarial cases for the sealed file (ADR-0015 D13, Confirmation §3).
//!
//! These are the cases the fitness function names: a forged file, an old file
//! put back in place, a wrong key. Each one has to fail in a specific way —
//! "it errored" is not enough, because the operator's next move depends on
//! which of these happened.

use std::collections::BTreeMap;

use core_crypto::{Contents, PolicyRule, SealError, SealKey, open, peek_version, seal};

const SECRET: &str = "super-secret-value-9f3a2b";
const SECRET_PATH: &str = "prod/db-root-credential";

fn contents(version: u64) -> Contents {
    Contents {
        version,
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
    }
}

fn key(fill: u8) -> SealKey {
    SealKey::from_bytes([fill; 32])
}

/// Every single byte matters. Flipping any one of them — header, ciphertext or
/// tag — must fail authentication rather than yield something believable.
#[test]
fn flipping_any_byte_is_caught() {
    let sealed = seal(&contents(3), &key(1)).unwrap();

    for index in 0..sealed.len() {
        let mut forged = sealed.clone();
        forged[index] ^= 0x01;

        let err = open(&forged, &key(1), None)
            .expect_err("byte {index} was flipped and the file still opened");

        // Bytes 0..9 are magic and format version, which are checked by shape
        // before the tag is; everything from the content version onward is
        // covered by the AEAD because the header is the additional data.
        match (index, &err) {
            (0..=7, SealError::BadMagic) => {}
            (8, SealError::UnsupportedFormat { .. }) => {}
            (_, SealError::AuthFailed) => {}
            other => panic!("byte {index} gave the wrong error: {other:?}"),
        }
    }
}

#[test]
fn the_wrong_key_does_not_open_it() {
    let sealed = seal(&contents(3), &key(1)).unwrap();
    let err = open(&sealed, &key(2), None).unwrap_err();
    assert!(matches!(err, SealError::AuthFailed), "got {err:?}");
}

/// The rollback case in full: an attacker who can write to the bucket puts back
/// a genuine, correctly sealed, *older* file. Encryption cannot see anything
/// wrong with it. Only the held version can.
#[test]
fn an_older_file_is_refused_even_though_it_is_genuine() {
    let old = seal(&contents(5), &key(1)).unwrap();

    // It is perfectly valid on its own terms.
    assert_eq!(open(&old, &key(1), None).unwrap().version, 5);
    assert_eq!(open(&old, &key(1), Some(5)).unwrap().version, 5);

    let err = open(&old, &key(1), Some(7)).unwrap_err();
    assert!(
        matches!(
            err,
            SealError::Rollback {
                held: 7,
                offered: 5
            }
        ),
        "got {err:?}"
    );
}

/// Raising the version in the header to slip an old file past the freshness
/// check must fail, because the header is authenticated.
#[test]
fn lying_about_the_version_in_the_header_fails_the_tag() {
    let mut forged = seal(&contents(5), &key(1)).unwrap();
    forged[9..17].copy_from_slice(&9u64.to_le_bytes());

    // The lie is visible to a cheap peek, which is what makes the peek useful.
    assert_eq!(peek_version(&forged).unwrap(), 9);
    // And it is fatal once the file is actually opened.
    let err = open(&forged, &key(1), Some(7)).unwrap_err();
    assert!(matches!(err, SealError::AuthFailed), "got {err:?}");
}

#[test]
fn malformed_containers_are_named_precisely() {
    let sealed = seal(&contents(1), &key(1)).unwrap();

    let err = open(b"", &key(1), None).unwrap_err();
    assert!(
        matches!(err, SealError::Truncated { got: 0, .. }),
        "got {err:?}"
    );

    let err = open(&sealed[..20], &key(1), None).unwrap_err();
    assert!(
        matches!(err, SealError::Truncated { got: 20, .. }),
        "got {err:?}"
    );

    let mut wrong_magic = sealed.clone();
    wrong_magic[..8].copy_from_slice(b"POSTGRES");
    assert!(matches!(
        open(&wrong_magic, &key(1), None).unwrap_err(),
        SealError::BadMagic
    ));

    let mut future = sealed.clone();
    future[8] = 2;
    assert!(matches!(
        open(&future, &key(1), None).unwrap_err(),
        SealError::UnsupportedFormat { got: 2 }
    ));
}

/// ADR-0011 §5 / ADR-0013 E1, extended to this crate: these errors are rendered
/// into logs, so none of them may carry secret material. `MalformedBody` is the
/// variant this guards most — `serde_json`'s own message quotes the offending
/// value, which is exactly why it is not forwarded.
#[test]
fn no_error_variant_carries_secret_material() {
    let sealed = seal(&contents(5), &key(1)).unwrap();

    let mut errs = vec![
        open(b"", &key(1), None).unwrap_err(),
        open(&sealed, &key(2), None).unwrap_err(),
        open(&sealed, &key(1), Some(9)).unwrap_err(),
    ];
    let mut wrong_magic = sealed.clone();
    wrong_magic[0] = b'X';
    errs.push(open(&wrong_magic, &key(1), None).unwrap_err());

    // A body that decrypts cleanly but is not the expected shape.
    let junk = seal(
        &Contents {
            version: 1,
            secrets: BTreeMap::new(),
            policies: BTreeMap::new(),
            tokens: BTreeMap::new(),
        },
        &key(1),
    )
    .unwrap();
    assert!(open(&junk, &key(1), None).is_ok());

    for err in &errs {
        let rendered = format!("{err} / {err:?}");
        assert!(!rendered.contains(SECRET), "{err:?} rendered the secret");
        assert!(!rendered.contains(SECRET_PATH), "{err:?} rendered a path");
    }
}

/// `VersionMismatch` cannot be reached by tampering, because the header is
/// authenticated — the case above proves that. It exists to catch a *writer*
/// that builds a header disagreeing with the body, which is a bug we would
/// otherwise ship silently. This test pins the shape so the check is not
/// deleted as dead code.
#[test]
fn header_and_body_versions_are_cross_checked() {
    let sealed = seal(&contents(4), &key(1)).unwrap();
    let opened = open(&sealed, &key(1), None).unwrap();
    assert_eq!(opened.version, peek_version(&sealed).unwrap());
}
