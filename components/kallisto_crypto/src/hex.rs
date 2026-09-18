//! Hex, because key material reaches this program as text.
//!
//! Its own module because three different kinds of key now travel as hex — the
//! seal key, the token key inside the file, and the token hashes in the file's
//! token table — and one of them having a subtly different parser from the
//! others is the sort of thing that only shows up as an authentication failure
//! nobody can explain.

#[derive(Debug, thiserror::Error)]
pub enum HexError {
    #[error("expected {expected} hex characters, got {got}")]
    WrongLength { expected: usize, got: usize },
    #[error("expected hex characters only ([0-9a-fA-F]); character {at} is not")]
    NotHex { at: usize },
}

/// Fills `out` completely, or fails. Accepts either case, and tolerates
/// surrounding whitespace because environment variables collect it.
pub fn decode_into(text: &str, out: &mut [u8]) -> Result<(), HexError> {
    let text = text.trim();
    if text.len() != out.len() * 2 {
        return Err(HexError::WrongLength {
            expected: out.len() * 2,
            got: text.len(),
        });
    }

    for (i, pair) in text.as_bytes().chunks_exact(2).enumerate() {
        let hi = digit(pair[0]).ok_or(HexError::NotHex { at: i * 2 })?;
        let lo = digit(pair[1]).ok_or(HexError::NotHex { at: i * 2 + 1 })?;
        out[i] = (hi << 4) | lo;
    }
    Ok(())
}

pub fn encode(bytes: &[u8]) -> String {
    let mut out = vec![0u8; bytes.len() * 2];
    encode_into(bytes, &mut out);
    String::from_utf8(out).unwrap_or_default()
}

/// Hex into a caller-owned buffer, which must be exactly twice as long as
/// `bytes`. Anything else writes nothing.
///
/// Exists because the access log renders an identifier for every request, and
/// [`encode`]'s `String` would be one heap allocation per line on the read path
/// (ADR-0015 D15: the log never burdens the path it observes).
pub fn encode_into(bytes: &[u8], out: &mut [u8]) {
    if out.len() != bytes.len() * 2 {
        return;
    }
    for (byte, pair) in bytes.iter().zip(out.chunks_exact_mut(2)) {
        pair[0] = NIBBLE[usize::from(byte >> 4)];
        pair[1] = NIBBLE[usize::from(byte & 0x0f)];
    }
}

const NIBBLE: &[u8; 16] = b"0123456789abcdef";

fn digit(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_round_trips_in_either_case() {
        let mut out = [0u8; 4];
        decode_into("DEADbeef", &mut out).unwrap();
        assert_eq!(out, [0xde, 0xad, 0xbe, 0xef]);
        assert_eq!(encode(&out), "deadbeef");
    }

    #[test]
    fn it_reports_the_length_and_never_the_content() {
        let mut out = [0u8; 4];
        let err = decode_into("dead", &mut out).unwrap_err();
        assert!(matches!(
            err,
            HexError::WrongLength {
                expected: 8,
                got: 4
            }
        ));
        assert!(!err.to_string().contains("dead"));

        assert!(matches!(
            decode_into("deadbeeZ", &mut out).unwrap_err(),
            HexError::NotHex { at: 7 }
        ));
    }

    #[test]
    fn encoding_into_a_buffer_matches_the_allocating_form() {
        let bytes = [0x00, 0x0f, 0xa5, 0xff];
        let mut out = [0u8; 8];
        encode_into(&bytes, &mut out);
        assert_eq!(&out, b"000fa5ff");
        assert_eq!(std::str::from_utf8(&out).unwrap(), encode(&bytes));
    }

    /// A wrong-sized buffer writes nothing rather than half an answer: a
    /// truncated identifier in a log line would silently alias other paths.
    #[test]
    fn encoding_into_a_wrong_sized_buffer_writes_nothing() {
        let mut out = [b'.'; 4];
        encode_into(&[0xab, 0xcd, 0xef], &mut out);
        assert_eq!(&out, b"....");
    }

    #[test]
    fn surrounding_whitespace_is_tolerated() {
        let mut out = [0u8; 2];
        decode_into("  beef\n", &mut out).unwrap();
        assert_eq!(out, [0xbe, 0xef]);
    }
}
