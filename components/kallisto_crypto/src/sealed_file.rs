//! The on-bucket file format, and the only two operations Kallisto performs on
//! it (ADR-0015 D13, amended by ADR-0016 QĐ-1 for the algorithm choice).
//!
//! ```text
//! offset  size  field
//! 0       8     magic, b"KALLISTO"
//! 8       1     format version
//! 9       8     content version, u64 little-endian
//! 17      12    nonce
//! 29      ..    ciphertext, then a 16-byte tag
//! ```
//!
//! The header is plaintext so that [`peek_version`] can reject a stale file
//! without spending a decryption, but it is fed to the AEAD as additional
//! authenticated data, so editing any of it — the content version most of all —
//! fails the tag check. That is the whole anti-rollback mechanism: encryption
//! stops a forged file, and the counter stops a genuine *old* file being put
//! back in place with revoked tokens and rotated keys still in it.

use std::{collections::BTreeMap, fmt};

use aws_lc_rs::{
    aead::{AES_256_GCM, Aad, LessSafeKey, NONCE_LEN, Nonce, UnboundKey},
    rand::{SecureRandom, SystemRandom},
};
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use zeroize::Zeroizing;

use crate::key::SealKey;

pub const MAGIC: [u8; 8] = *b"KALLISTO";
pub const FORMAT_VERSION: u8 = 1;
pub const HEADER_LEN: usize = MAGIC.len() + 1 + 8 + NONCE_LEN;

const VERSION_AT: usize = MAGIC.len() + 1;
const NONCE_AT: usize = VERSION_AT + 8;

#[derive(Debug, thiserror::Error)]
pub enum SealError {
    #[error("not a Kallisto sealed file")]
    BadMagic,
    #[error(
        "sealed file format version {got} is not {FORMAT_VERSION}, which is all this build reads"
    )]
    UnsupportedFormat { got: u8 },
    #[error("sealed file is truncated: {got} bytes, need at least {need}")]
    Truncated { got: usize, need: usize },
    #[error("refusing an older sealed file: holding version {held}, offered {offered}")]
    Rollback { held: u64, offered: u64 },
    #[error("sealed file failed authentication")]
    AuthFailed,
    // Deliberately carries no detail. `serde_json`'s own message quotes the
    // offending value, and the offending value here is secret material.
    #[error("sealed file body is not the shape this build expects")]
    MalformedBody,
    #[error("header says version {header}, body says {body}")]
    VersionMismatch { header: u64, body: u64 },
    #[error("system random number generator is unavailable")]
    RandomUnavailable,
}

/// One rule inside a policy. Kept here rather than in `policy_engine` because
/// it is part of the *file format*, and the format has exactly one owner.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyRule {
    /// Vault path syntax, `data/` quirk and all: `secret/data/payment/*` grants
    /// reads, `secret/metadata/payment/*` grants lists (ADR-0015 D8).
    pub path: String,
    pub capabilities: Vec<String>,
}

/// The decrypted body.
///
/// Short-lived by design: it exists inside the refresh loop only, long enough
/// to build a snapshot, and ADR-0015 D13's in-memory barrier re-seals every
/// secret before anything is served. Nothing may hold one of these across a
/// request.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Contents {
    /// Must equal the header's content version. Monotonic; never reused.
    pub version: u64,
    /// Secret path (without the `secret/data/` prefix) to its KV-v2 data
    /// object.
    #[serde(default)]
    pub secrets: BTreeMap<String, serde_json::Value>,
    /// Policy name to its rules.
    #[serde(default)]
    pub policies: BTreeMap<String, Vec<PolicyRule>>,
    /// Keyed hash of a token to the policy names it carries (ADR-0015 D8).
    #[serde(default)]
    pub tokens: BTreeMap<String, Vec<String>>,
    /// The key the hashes in `tokens` were computed with, hex-encoded.
    ///
    /// It lives *inside* the file rather than being derived from the seal key,
    /// and that is the whole point: rotating the seal key then re-seals the
    /// same table and every token keeps working. Derive it from the seal key
    /// instead and a key rotation silently invalidates every token in the
    /// fleet — the operator holds the hashes, not the tokens, so there would be
    /// no way to recompute them.
    ///
    /// Absent means no token authentication (ADR-0015 D8's sidecar case, where
    /// the bucket credential is the boundary). Absent *with* a non-empty
    /// `tokens` table is a malformed file, and the resolver refuses it rather
    /// than serving with authorization silently switched off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_key: Option<String>,
}

/// Counts, never contents. A `{:?}` on this reaches logs and panic messages.
impl fmt::Debug for Contents {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Contents")
            .field("version", &self.version)
            .field(
                "secrets",
                &format_args!("<{} REDACTED>", self.secrets.len()),
            )
            .field("policies", &format_args!("<{} names>", self.policies.len()))
            .field("tokens", &format_args!("<{} entries>", self.tokens.len()))
            .field("token_key", &"<REDACTED>")
            .finish()
    }
}

struct Header {
    content_version: u64,
    nonce: [u8; NONCE_LEN],
}

impl Header {
    fn parse(bytes: &[u8]) -> Result<Self, SealError> {
        if bytes.len() < HEADER_LEN {
            return Err(SealError::Truncated {
                got: bytes.len(),
                need: HEADER_LEN,
            });
        }
        if bytes[..MAGIC.len()] != MAGIC {
            return Err(SealError::BadMagic);
        }
        let got = bytes[MAGIC.len()];
        if got != FORMAT_VERSION {
            return Err(SealError::UnsupportedFormat { got });
        }

        let mut version = [0u8; 8];
        version.copy_from_slice(&bytes[VERSION_AT..NONCE_AT]);
        let mut nonce = [0u8; NONCE_LEN];
        nonce.copy_from_slice(&bytes[NONCE_AT..HEADER_LEN]);

        Ok(Self {
            content_version: u64::from_le_bytes(version),
            nonce,
        })
    }

    fn write(content_version: u64, nonce: &[u8; NONCE_LEN]) -> [u8; HEADER_LEN] {
        let mut header = [0u8; HEADER_LEN];
        header[..MAGIC.len()].copy_from_slice(&MAGIC);
        header[MAGIC.len()] = FORMAT_VERSION;
        header[VERSION_AT..NONCE_AT].copy_from_slice(&content_version.to_le_bytes());
        header[NONCE_AT..].copy_from_slice(nonce);
        header
    }
}

/// Reads the content version without decrypting anything.
///
/// Cheap enough to call on every poll. The number is only *trusted* once
/// [`open`] has authenticated the header, so a caller that acts on it before
/// that must be acting conservatively — refusing, never accepting.
pub fn peek_version(bytes: &[u8]) -> Result<u64, SealError> {
    Ok(Header::parse(bytes)?.content_version)
}

/// Seals `contents` under `key`. The header's content version is taken from
/// `contents.version`, so the two can never disagree on the writing side.
pub fn seal(contents: &Contents, key: &SealKey) -> Result<Vec<u8>, SealError> {
    let mut body =
        Zeroizing::new(serde_json::to_vec(contents).map_err(|_| SealError::MalformedBody)?);

    let mut nonce = [0u8; NONCE_LEN];
    SystemRandom::new()
        .fill(&mut nonce)
        .map_err(|_| SealError::RandomUnavailable)?;

    let header = Header::write(contents.version, &nonce);
    aead(key)?
        .seal_in_place_append_tag(
            Nonce::assume_unique_for_key(nonce),
            Aad::from(header),
            &mut *body,
        )
        .map_err(|_| SealError::AuthFailed)?;

    let mut sealed = Vec::with_capacity(HEADER_LEN + body.len());
    sealed.extend_from_slice(&header);
    sealed.extend_from_slice(&body);
    Ok(sealed)
}

/// Opens and authenticates a sealed file.
///
/// `held` is the content version this process is already serving, if any.
/// Anything older is refused before the decryption runs — see the module
/// comment for why that check cannot live in the bucket instead.
///
/// Returns the plaintext still in its own wiped-on-drop buffer rather than a
/// parsed structure. See [`Opened`] for why that distinction is the whole
/// point.
pub fn open(bytes: &[u8], key: &SealKey, held: Option<u64>) -> Result<Opened, SealError> {
    let header = Header::parse(bytes)?;
    if let Some(held) = held
        && header.content_version < held
    {
        return Err(SealError::Rollback {
            held,
            offered: header.content_version,
        });
    }

    let mut in_out = Zeroizing::new(bytes[HEADER_LEN..].to_vec());
    let aad: [u8; HEADER_LEN] = Header::write(header.content_version, &header.nonce);
    let plain_len = aead(key)?
        .open_in_place(
            Nonce::assume_unique_for_key(header.nonce),
            Aad::from(aad),
            &mut in_out,
        )
        .map_err(|_| SealError::AuthFailed)?
        .len();
    in_out.truncate(plain_len);

    let opened = Opened {
        plaintext: in_out,
        content_version: header.content_version,
    };
    // Parse once here purely to reject a body that disagrees with its own
    // header before the caller ever sees it. The borrowed view is dropped
    // immediately; nothing it points at outlives `opened`.
    let version = opened.view()?.version;
    if version != header.content_version {
        return Err(SealError::VersionMismatch {
            header: header.content_version,
            body: version,
        });
    }
    Ok(opened)
}

/// An authenticated file's plaintext, still in the buffer that wipes itself.
///
/// The reason this type exists instead of `open` simply returning [`Contents`]:
/// `serde_json` deserialising into owned `String`s and `Value`s makes a second
/// copy of every secret in the file, on the heap, which nothing zeroes when it
/// is dropped. That copy survives in freed memory, and freed memory is exactly
/// what ends up in a core dump or a swapped-out page — the two accidents
/// ADR-0015 D13's RAM half exists to prevent. Re-sealing each secret into an
/// in-memory barrier while leaving that copy behind would have been decoration.
///
/// So the plaintext stays in one `Zeroizing` buffer, [`Self::view`] hands out
/// borrows into it, and the caller re-seals straight from those borrows. The
/// only copy of a secret that outlives this buffer is the sealed one.
pub struct Opened {
    plaintext: Zeroizing<Vec<u8>>,
    content_version: u64,
}

impl Opened {
    pub fn content_version(&self) -> u64 {
        self.content_version
    }

    pub fn view(&self) -> Result<View<'_>, SealError> {
        serde_json::from_slice(&self.plaintext).map_err(|_| SealError::MalformedBody)
    }
}

/// Counts nothing, because it owns nothing worth counting — but a `{:?}` on it
/// would otherwise print the whole file.
impl fmt::Debug for Opened {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Opened")
            .field("content_version", &self.content_version)
            .field(
                "plaintext",
                &format_args!("<{} REDACTED>", self.plaintext.len()),
            )
            .finish()
    }
}

/// The decrypted body, borrowed rather than owned.
///
/// Secret values stay as [`RawValue`] — the exact JSON text as it appears in
/// the file, pointing into the buffer that wipes itself. Nothing re-serialises
/// them, which removes a copy *and* removes the chance of a re-render that
/// differs from what the operator wrote.
///
/// `policies` and `tokens` are owned, deliberately: they hold path patterns,
/// policy names and keyed hashes, none of which is secret material. The token
/// *key* is borrowed, because it is.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct View<'a> {
    pub version: u64,
    #[serde(default, borrow)]
    pub secrets: BTreeMap<&'a str, &'a RawValue>,
    #[serde(default)]
    pub policies: BTreeMap<String, Vec<PolicyRule>>,
    #[serde(default)]
    pub tokens: BTreeMap<String, Vec<String>>,
    #[serde(default, borrow)]
    pub token_key: Option<&'a str>,
}

fn aead(key: &SealKey) -> Result<LessSafeKey, SealError> {
    let unbound = UnboundKey::new(&AES_256_GCM, key.expose()).map_err(|_| SealError::AuthFailed)?;
    Ok(LessSafeKey::new(unbound))
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn sample() -> Contents {
        Contents {
            version: 7,
            secrets: BTreeMap::from([(
                "app/db".to_string(),
                serde_json::json!({"user": "admin", "pass": "duck-fixture-not-a-credential"}),
            )]),
            policies: BTreeMap::from([(
                "payment".to_string(),
                vec![PolicyRule {
                    path: "secret/data/payment/*".to_string(),
                    capabilities: vec!["read".to_string()],
                }],
            )]),
            tokens: BTreeMap::from([("deadbeef".to_string(), vec!["payment".to_string()])]),
            token_key: Some("ab".repeat(32)),
        }
    }

    fn key() -> SealKey {
        SealKey::from_bytes([9u8; 32])
    }

    #[test]
    fn round_trips() {
        let sealed = seal(&sample(), &key()).unwrap();
        assert_eq!(&sealed[..MAGIC.len()], &MAGIC);
        assert_eq!(peek_version(&sealed).unwrap(), 7);
        let opened = open(&sealed, &key(), None).unwrap();
        let view = opened.view().unwrap();
        assert_eq!(view.version, 7);
        assert_eq!(
            view.secrets["app/db"].get(),
            r#"{"pass":"duck-fixture-not-a-credential","user":"admin"}"#
        );
        assert_eq!(view.token_key, Some("ab".repeat(32).as_str()));
    }

    /// Sealing the same contents twice must not produce the same bytes, or the
    /// nonce is not random and the whole construction is unsound.
    #[test]
    fn two_seals_differ() {
        let a = seal(&sample(), &key()).unwrap();
        let b = seal(&sample(), &key()).unwrap();
        assert_ne!(a, b);
        assert_eq!(a[..HEADER_LEN - NONCE_LEN], b[..HEADER_LEN - NONCE_LEN]);
    }

    /// The path name must stay inside the ciphertext. SOPS leaks key names;
    /// ADR-0015 D13 sealed the whole file specifically so Kallisto does not.
    #[test]
    fn nothing_readable_survives_into_the_sealed_bytes() {
        let sealed = seal(&sample(), &key()).unwrap();
        for needle in [
            "app/db",
            "duck-fixture-not-a-credential",
            "payment",
            "deadbeef",
        ] {
            assert!(
                !sealed.windows(needle.len()).any(|w| w == needle.as_bytes()),
                "{needle:?} is readable in the sealed file"
            );
        }
    }

    #[test]
    fn debug_shows_counts_not_contents() {
        let rendered = format!("{:?}", sample());
        assert!(
            !rendered.contains("duck-fixture-not-a-credential"),
            "Debug leaked a secret: {rendered}"
        );
        assert!(
            !rendered.contains("app/db"),
            "Debug leaked a path: {rendered}"
        );
        assert!(rendered.contains("version: 7"));
        assert!(rendered.contains("REDACTED"));
    }
}
