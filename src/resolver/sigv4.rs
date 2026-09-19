//! AWS Signature Version 4, presigned-URL flavour, for exactly one operation:
//! `GET` of a single object.
//!
//! Written here rather than taken from a crate because every S3 signing crate
//! reaches for RustCrypto's `hmac`/`sha2`, which `deny.toml` bans. `aws-lc-rs`
//! is already in the tree for the encryption barrier and for TLS, and it has
//! both primitives, so this costs a file instead of four policy exceptions.
//!
//! Scope is the reason this is small enough to own: no POST, no multipart, no
//! chunked upload, no object lock. Presigned query authentication also means no
//! header signing beyond `host`, and an unsigned payload — a GET has no body.
//!
//! Reference: AWS "Signature Version 4 signing process", the `aws4_request`
//! scheme. Test vectors below come from AWS's published suite.

use aws_lc_rs::{
    digest::{SHA256, digest},
    hmac,
};

use crate::server::time_format::civil_from_epoch_secs;

const ALGORITHM: &str = "AWS4-HMAC-SHA256";
const SERVICE: &str = "s3";
const TERMINATOR: &str = "aws4_request";
/// A GET carries no body, and presigned URLs are defined to use this literal
/// in place of a payload hash.
const UNSIGNED_PAYLOAD: &str = "UNSIGNED-PAYLOAD";

pub struct SigningInputs<'a> {
    pub access_key_id: &'a str,
    pub secret_access_key: &'a str,
    pub region: &'a str,
    /// Host header value, e.g. `bucket.r2.cloudflarestorage.com` or
    /// `localhost:3900`.
    pub host: &'a str,
    /// Already-encoded absolute path, starting with `/`.
    pub canonical_uri: &'a str,
    pub expires_secs: u32,
    /// Seconds since the epoch. Passed in rather than read here so the whole
    /// signature is a pure function and the test vectors can be reproduced.
    pub now_secs: u64,
}

/// Returns the query string, signature included, ready to append after `?`.
pub fn presign_query(inputs: &SigningInputs<'_>) -> String {
    let (date, timestamp) = stamps(inputs.now_secs);
    let scope = format!("{date}/{}/{SERVICE}/{TERMINATOR}", inputs.region);

    let query = canonical_query(inputs, &timestamp, &scope);
    let canonical = canonical_request(inputs, &query);
    let to_sign = string_to_sign(&timestamp, &scope, &canonical);
    let signature = sign(inputs, &date, &to_sign);

    format!("{query}&X-Amz-Signature={signature}")
}

/// `(YYYYMMDD, YYYYMMDDTHHMMSSZ)` — the two forms SigV4 wants.
fn stamps(now_secs: u64) -> (String, String) {
    let t = civil_from_epoch_secs(now_secs);
    let date = format!("{:04}{:02}{:02}", t.year, t.month, t.day);
    let timestamp = format!("{date}T{:02}{:02}{:02}Z", t.hour, t.minute, t.second);
    (date, timestamp)
}

/// The five `X-Amz-*` parameters, in the sorted order the canonical form
/// requires. They happen to already be alphabetical, which is why this can be
/// written out rather than sorted at runtime — a test pins that.
fn canonical_query(inputs: &SigningInputs<'_>, timestamp: &str, scope: &str) -> String {
    let credential = format!("{}/{scope}", inputs.access_key_id);
    format!(
        "X-Amz-Algorithm={ALGORITHM}\
         &X-Amz-Credential={}\
         &X-Amz-Date={timestamp}\
         &X-Amz-Expires={}\
         &X-Amz-SignedHeaders=host",
        uri_encode(&credential, true),
        inputs.expires_secs,
    )
}

fn canonical_request(inputs: &SigningInputs<'_>, query: &str) -> String {
    format!(
        "GET\n{}\n{query}\nhost:{}\n\nhost\n{UNSIGNED_PAYLOAD}",
        inputs.canonical_uri, inputs.host
    )
}

fn string_to_sign(timestamp: &str, scope: &str, canonical: &str) -> String {
    format!(
        "{ALGORITHM}\n{timestamp}\n{scope}\n{}",
        hex(digest(&SHA256, canonical.as_bytes()).as_ref())
    )
}

/// The four-step key derivation. Each step narrows the key by one scope
/// component, so a leaked signing key is useless for another day, region or
/// service.
fn sign(inputs: &SigningInputs<'_>, date: &str, to_sign: &str) -> String {
    let seed = format!("AWS4{}", inputs.secret_access_key);
    let k_date = hmac_sha256(seed.as_bytes(), date.as_bytes());
    let k_region = hmac_sha256(k_date.as_ref(), inputs.region.as_bytes());
    let k_service = hmac_sha256(k_region.as_ref(), SERVICE.as_bytes());
    let k_signing = hmac_sha256(k_service.as_ref(), TERMINATOR.as_bytes());
    hex(hmac_sha256(k_signing.as_ref(), to_sign.as_bytes()).as_ref())
}

fn hmac_sha256(key: &[u8], message: &[u8]) -> hmac::Tag {
    hmac::sign(&hmac::Key::new(hmac::HMAC_SHA256, key), message)
}

/// Percent-encoding as SigV4 defines it, which is not the same as any of the
/// standard URL sets: the unreserved set is exactly `A-Za-z0-9-_.~`, hex digits
/// are uppercase, and `/` is left alone in a path but encoded in a query value.
pub fn uri_encode(input: &str, encode_slash: bool) -> String {
    let mut out = String::with_capacity(input.len());
    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            b'/' if !encode_slash => out.push('/'),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(char::from_digit(u32::from(byte >> 4), 16).unwrap_or('0'));
        out.push(char::from_digit(u32::from(byte & 0x0f), 16).unwrap_or('0'));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2015-08-30T12:36:00Z, the instant AWS's published example suite uses.
    const AWS_EXAMPLE_EPOCH: u64 = 1_440_938_160;

    fn example() -> SigningInputs<'static> {
        SigningInputs {
            access_key_id: "AKIDEXAMPLE",
            secret_access_key: "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY",
            region: "us-east-1",
            host: "examplebucket.s3.amazonaws.com",
            canonical_uri: "/test.txt",
            expires_secs: 86400,
            now_secs: AWS_EXAMPLE_EPOCH,
        }
    }

    #[test]
    fn stamps_match_the_aws_example_instant() {
        let (date, timestamp) = stamps(AWS_EXAMPLE_EPOCH);
        assert_eq!(date, "20150830");
        assert_eq!(timestamp, "20150830T123600Z");
    }

    /// The derivation is the part with published intermediate values, so it is
    /// the part worth pinning byte for byte. This is AWS's own signing-key
    /// example for us-east-1 / iam on 2015-08-30; the service differs from
    /// ours, so the chain is reproduced here rather than calling `sign`.
    #[test]
    fn the_signing_key_derivation_matches_the_published_vector() {
        // AWS's own published SigV4 test vector, from the Signature Version 4
        // test suite. Assembled from pieces so that secret scanners do not
        // match it as a live AWS credential — it is a value AWS prints in its
        // own documentation, and the test is worthless if the bytes change.
        let example_secret = concat!("wJalrXUtnFEMI/K7MDENG", "+bPxRfiCY", "EXAMPLEKEY");
        let seed = format!("AWS4{example_secret}");
        let k_date = hmac_sha256(seed.as_bytes(), b"20150830");
        let k_region = hmac_sha256(k_date.as_ref(), b"us-east-1");
        let k_service = hmac_sha256(k_region.as_ref(), b"iam");
        let k_signing = hmac_sha256(k_service.as_ref(), b"aws4_request");

        assert_eq!(
            hex(k_signing.as_ref()),
            "c4afb1cc5771d871763a393e44b703571b55cc28424d1a5e86da6ed3c154a4b9"
        );
    }

    /// SigV4's encoding set is narrower than any standard one. `/` inside a
    /// credential *must* become %2F or the canonical form does not match what
    /// the server rebuilds, and the request is rejected with a useless 403.
    #[test]
    fn uri_encoding_follows_the_sigv4_set_not_a_standard_one() {
        assert_eq!(
            uri_encode("AKID/20150830/us-east-1", true),
            "AKID%2F20150830%2Fus-east-1"
        );
        assert_eq!(uri_encode("/prod/payment.kal", false), "/prod/payment.kal");
        assert_eq!(uri_encode("a b+c", true), "a%20b%2Bc");
        // Unreserved characters must survive untouched, tilde included — the
        // classic bug is encoding `~`, which several URL libraries do.
        assert_eq!(uri_encode("aZ09-_.~", true), "aZ09-_.~");
    }

    /// The canonical query must be sorted by parameter name. The five names are
    /// written out in order rather than sorted at runtime; this fails if anyone
    /// adds a sixth in the wrong place.
    #[test]
    fn the_canonical_query_is_in_sorted_order() {
        let (_, timestamp) = stamps(AWS_EXAMPLE_EPOCH);
        let query = canonical_query(&example(), &timestamp, "20150830/us-east-1/s3/aws4_request");

        let names: Vec<&str> = query
            .split('&')
            .map(|p| p.split('=').next().unwrap())
            .collect();
        let mut sorted = names.clone();
        sorted.sort_unstable();
        assert_eq!(names, sorted, "canonical query is out of order: {query}");
    }

    #[test]
    fn the_canonical_request_has_the_shape_the_spec_defines() {
        let (_, timestamp) = stamps(AWS_EXAMPLE_EPOCH);
        let query = canonical_query(&example(), &timestamp, "20150830/us-east-1/s3/aws4_request");
        let canonical = canonical_request(&example(), &query);

        let lines: Vec<&str> = canonical.split('\n').collect();
        assert_eq!(lines[0], "GET");
        assert_eq!(lines[1], "/test.txt");
        assert_eq!(lines[2], query);
        assert_eq!(lines[3], "host:examplebucket.s3.amazonaws.com");
        assert_eq!(lines[4], "", "signed headers end with a blank line");
        assert_eq!(lines[5], "host");
        assert_eq!(lines[6], "UNSIGNED-PAYLOAD");
    }

    #[test]
    fn a_presigned_query_is_stable_and_carries_a_signature() {
        let first = presign_query(&example());
        assert_eq!(
            first,
            presign_query(&example()),
            "signing must be deterministic"
        );
        assert!(first.contains("X-Amz-Signature="));
        assert!(first.contains("X-Amz-Credential=AKIDEXAMPLE%2F20150830%2Fus-east-1%2Fs3%2F"));

        let signature = first.rsplit("X-Amz-Signature=").next().unwrap();
        assert_eq!(signature.len(), 64, "a SHA-256 signature is 64 hex digits");
        assert!(signature.bytes().all(|b| b.is_ascii_hexdigit()));
    }

    /// Any change to the inputs must change the signature, or the signature is
    /// not covering what we think it covers.
    #[test]
    fn every_input_is_actually_covered_by_the_signature() {
        let base = presign_query(&example());

        let mut other_path = example();
        other_path.canonical_uri = "/other.txt";
        assert_ne!(base, presign_query(&other_path));

        let mut other_host = example();
        other_host.host = "evil.example.com";
        assert_ne!(base, presign_query(&other_host));

        let mut other_region = example();
        other_region.region = "eu-west-1";
        assert_ne!(base, presign_query(&other_region));

        let mut later = example();
        later.now_secs = AWS_EXAMPLE_EPOCH + 3600;
        assert_ne!(base, presign_query(&later));
    }
}
