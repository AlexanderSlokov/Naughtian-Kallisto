//! The keyed hash every identifier in a log line goes through.
//!
//! ADR-0015 D15 asks for **keyed** hashing, not a bare digest, and says so for
//! a concrete reason: a deployment has a few dozen paths and they are guessable
//! (`app/db`, `prod/stripe`). A bare SHA-256 of that set is reversed by
//! computing the same few dozen digests. A key the attacker does not hold
//! removes that, and this constraint applies to the error log exactly as much
//! as to the access log — a path leaking through a stack trace has leaked.

use aws_lc_rs::hmac::{self, HMAC_SHA256};
use core_crypto::hex;
use policy_engine::{LOG_LABEL, TokenKey};
use zeroize::{Zeroize, ZeroizeOnDrop};

pub const LOG_KEY_LEN: usize = 32;
/// The first 16 hex characters of the hash. Full 32-byte identifiers make a log
/// line mostly hash, and 64 bits is far past the point where a few dozen paths
/// collide.
pub const ID_LEN: usize = 16;

/// The key path identifiers are computed under.
///
/// Derived from the sealed file's token key (QĐ-7), so the identifiers mean the
/// same thing across a restart and an operator holding the file can work out
/// which path a line refers to. It is a *derived* key rather than the token key
/// itself because the input to this one is chosen by the caller — see
/// `LOG_LABEL` in `policy_engine`.
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct LogKey {
    raw: [u8; LOG_KEY_LEN],
    #[zeroize(skip)]
    prepared: hmac::Key,
}

impl LogKey {
    pub fn from_bytes(bytes: [u8; LOG_KEY_LEN]) -> Self {
        Self {
            prepared: hmac::Key::new(HMAC_SHA256, &bytes),
            raw: bytes,
        }
    }

    /// The file-derived key: stable for as long as the file's token key is.
    pub fn derived_from(token_key: &TokenKey) -> Self {
        Self::from_bytes(token_key.derive(LOG_LABEL))
    }

    /// The fallback for a file with no token table, and for requests refused
    /// before any file loaded (a 503 while sealed, a 429).
    ///
    /// Random per process, so identifiers correlate within one process lifetime
    /// and not beyond it. The server says so at startup rather than letting an
    /// operator assume otherwise.
    pub fn random() -> Self {
        let mut bytes = [0u8; LOG_KEY_LEN];
        aws_lc_rs::rand::fill(&mut bytes).expect("the system RNG must work");
        let key = Self::from_bytes(bytes);
        bytes.zeroize();
        key
    }

    /// `ID_LEN` hex characters identifying `value` under this key.
    pub fn id(&self, value: &str) -> Id {
        let tag = hmac::sign(&self.prepared, value.as_bytes());
        Id::from_bytes(tag.as_ref())
    }
}

/// Never the key.
impl std::fmt::Debug for LogKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("LogKey(<REDACTED>)")
    }
}

/// A rendered identifier: fixed width, inline, no allocation.
///
/// Inline because this is built on the read path for every request, and the
/// access log's whole claim is that it never blocks or burdens that path.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Id([u8; ID_LEN]);

impl Id {
    /// The placeholder for "there was nothing to identify here" — a request
    /// that carried no token, or one whose path never resolved to anything.
    pub const NONE: Self = Self([b'-'; ID_LEN]);

    pub fn from_bytes(bytes: &[u8]) -> Self {
        let mut out = [b'0'; ID_LEN];
        hex::encode_into(&bytes[..ID_LEN / 2], &mut out);
        Self(out)
    }

    pub fn as_str(&self) -> &str {
        // SAFETY-free: `hex::encode_into` only ever writes ASCII hex digits and
        // `NONE` is ASCII, so this is always valid UTF-8.
        std::str::from_utf8(&self.0).unwrap_or("?")
    }
}

impl std::fmt::Display for Id {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::fmt::Debug for Id {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn token_key() -> TokenKey {
        TokenKey::from_bytes([3u8; 32])
    }

    #[test]
    fn the_same_path_gets_the_same_identifier_under_one_key() {
        let key = LogKey::derived_from(&token_key());
        assert_eq!(key.id("app/db"), key.id("app/db"));
        assert_ne!(key.id("app/db"), key.id("app/web"));
    }

    /// The point of QĐ-7: restart the process, keep the file, and yesterday's
    /// log lines still line up with today's.
    #[test]
    fn the_identifier_survives_a_restart_because_the_key_is_in_the_file() {
        let first = LogKey::derived_from(&token_key());
        let second = LogKey::derived_from(&token_key());
        assert_eq!(first.id("app/db"), second.id("app/db"));
    }

    #[test]
    fn another_file_gives_another_identifier() {
        let a = LogKey::derived_from(&TokenKey::from_bytes([1u8; 32]));
        let b = LogKey::derived_from(&TokenKey::from_bytes([2u8; 32]));
        assert_ne!(a.id("app/db"), b.id("app/db"));
    }

    #[test]
    fn a_random_key_is_different_every_time() {
        assert_ne!(LogKey::random().id("app/db"), LogKey::random().id("app/db"));
    }

    /// QĐ-7's security condition, stated as a test. Paths are attacker-chosen;
    /// if they hashed under the token label, reading the log would hand out
    /// `HMAC(token_key, arbitrary)` and with it a reverse table against the
    /// file's token column.
    #[test]
    fn a_path_and_a_token_of_the_same_text_do_not_share_an_identifier() {
        let token_key = token_key();
        let log_key = LogKey::derived_from(&token_key);

        let text = "s.sometoken";
        let as_token = token_key.hash_hex(text);
        let as_path = log_key.id(text);

        assert!(
            !as_token.starts_with(as_path.as_str()),
            "the log key is not domain-separated from the token key"
        );
    }

    #[test]
    fn a_log_key_never_renders_itself() {
        let rendered = format!("{:?}", LogKey::from_bytes([0xab; 32]));
        assert!(rendered.contains("REDACTED"));
        assert!(!rendered.contains("abab"), "the key leaked: {rendered}");
    }

    #[test]
    fn an_identifier_is_fixed_width_hex_and_the_placeholder_is_not() {
        let id = LogKey::random().id("app/db");
        assert_eq!(id.as_str().len(), ID_LEN);
        assert!(id.as_str().chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(Id::NONE.as_str(), "----------------");
    }
}
