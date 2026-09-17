//! The 32-byte key that opens a sealed secret file.
//!
//! Kept in its own type rather than a bare `[u8; 32]` so that the two things
//! that always go wrong with key material cannot happen by accident: it zeroes
//! itself on drop, and its `Debug` does not print it. ADR-0015's threat model
//! is explicit that this does not stop an attacker with root reading process
//! memory — it stops the accidental leaks, and a key rendered into a log line
//! by a `{:?}` somewhere is exactly that.

use std::fmt;

use aws_lc_rs::rand::{SecureRandom, SystemRandom};
use zeroize::{Zeroize, ZeroizeOnDrop};

/// AES-256 takes a 256-bit key.
pub const KEY_LEN: usize = 32;

#[derive(Debug, thiserror::Error)]
pub enum KeyError {
    /// The length is the offending value; the content never is.
    #[error("key must be {KEY_LEN} bytes as {} hex characters, got {got} characters", KEY_LEN * 2)]
    WrongLength { got: usize },
    #[error("key must be hex characters only ([0-9a-fA-F]); byte {at} is not")]
    NotHex { at: usize },
    #[error("system random number generator is unavailable")]
    RandomUnavailable,
}

#[derive(Zeroize, ZeroizeOnDrop)]
pub struct SealKey([u8; KEY_LEN]);

impl SealKey {
    pub fn from_bytes(bytes: [u8; KEY_LEN]) -> Self {
        Self(bytes)
    }

    /// Parses the form an operator supplies: 64 hex characters, usually through
    /// an environment variable. Never accept a key as a command-line argument —
    /// arguments are visible to every process on the host through `ps`.
    pub fn from_hex(text: &str) -> Result<Self, KeyError> {
        let text = text.trim();
        if text.len() != KEY_LEN * 2 {
            return Err(KeyError::WrongLength { got: text.len() });
        }

        let mut bytes = [0u8; KEY_LEN];
        for (i, pair) in text.as_bytes().chunks_exact(2).enumerate() {
            let hi = hex_digit(pair[0]).ok_or(KeyError::NotHex { at: i * 2 })?;
            let lo = hex_digit(pair[1]).ok_or(KeyError::NotHex { at: i * 2 + 1 })?;
            bytes[i] = (hi << 4) | lo;
        }
        Ok(Self(bytes))
    }

    /// A fresh key from the system RNG. Used for the boot-time key of the
    /// in-memory barrier (ADR-0015 D13) and by the CLI when minting a new one.
    pub fn random() -> Result<Self, KeyError> {
        let mut bytes = [0u8; KEY_LEN];
        SystemRandom::new()
            .fill(&mut bytes)
            .map_err(|_| KeyError::RandomUnavailable)?;
        Ok(Self(bytes))
    }

    pub fn expose(&self) -> &[u8; KEY_LEN] {
        &self.0
    }

    /// Renders the key for an operator to store. The name is deliberately
    /// blunt: every call site should read as a decision to put a key somewhere.
    pub fn expose_as_hex(&self) -> String {
        let mut out = String::with_capacity(KEY_LEN * 2);
        for byte in &self.0 {
            out.push(char::from_digit(u32::from(byte >> 4), 16).unwrap_or('0'));
            out.push(char::from_digit(u32::from(byte & 0x0f), 16).unwrap_or('0'));
        }
        out
    }
}

fn hex_digit(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

impl fmt::Debug for SealKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SealKey(<REDACTED>)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";

    #[test]
    fn hex_round_trips() {
        let key = SealKey::from_hex(SAMPLE).unwrap();
        assert_eq!(key.expose()[0], 0x00);
        assert_eq!(key.expose()[31], 0x1f);
        assert_eq!(key.expose_as_hex(), SAMPLE);
    }

    #[test]
    fn uppercase_hex_is_accepted_and_whitespace_trimmed() {
        let key = SealKey::from_hex(&format!("  {}\n", SAMPLE.to_uppercase())).unwrap();
        assert_eq!(key.expose_as_hex(), SAMPLE);
    }

    #[test]
    fn wrong_length_and_non_hex_are_rejected() {
        SealKey::from_hex("abcd").unwrap_err();
        SealKey::from_hex(&"z".repeat(KEY_LEN * 2)).unwrap_err();
    }

    /// The key must not reach a log line through `{:?}`, including when it is
    /// nested inside another derived `Debug` — the way it normally would.
    #[test]
    fn debug_never_renders_the_key() {
        let key = SealKey::from_hex(SAMPLE).unwrap();
        let rendered = format!("{key:?} {key:#?}");
        assert!(!rendered.contains("0001020304"), "Debug leaked the key");
        assert!(rendered.contains("<REDACTED>"));

        #[derive(Debug)]
        #[allow(dead_code)]
        struct Holder {
            key: SealKey,
        }
        let nested = format!("{:?}", Holder { key });
        assert!(
            !nested.contains("0001020304"),
            "nested Debug leaked the key"
        );
    }

    #[test]
    fn random_keys_differ() {
        let a = SealKey::random().unwrap();
        let b = SealKey::random().unwrap();
        assert_ne!(a.expose(), b.expose());
    }

    /// The error text has to name the offending length without echoing the
    /// input, because the input here is key material.
    #[test]
    fn errors_do_not_echo_the_input() {
        let secretish = "f".repeat(10);
        let err = SealKey::from_hex(&secretish).unwrap_err();
        let rendered = format!("{err} / {err:?}");
        assert!(
            !rendered.contains(&secretish),
            "error echoed the input: {rendered}"
        );
        assert!(
            rendered.contains("10"),
            "error should name the bad length: {rendered}"
        );
    }
}
