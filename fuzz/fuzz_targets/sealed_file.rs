//! ADR-0013 V3, pointed at the highest-value surface in the program.
//!
//! The sealed file is the one input an attacker in ADR-0015 D13's threat model
//! controls completely: the model *is* somebody who can write to the bucket.
//! Every byte of it reaches `open` before a single authentication decision has
//! been made, and the header is parsed — magic, format version, content
//! version, nonce — before the tag is checked, because refusing a rollback
//! without spending a decryption is the whole point of that layout.
//!
//! So this target asks two things of arbitrary bytes:
//!
//! * `peek_version` must never panic on anything, including truncations of a
//!   real file. It runs on unauthenticated input by design.
//! * `open` must either fail or produce something, and never panic — with the
//!   right key as well as the wrong one, because a file that authenticates but
//!   carries malformed JSON is a case a bucket-writer can construct at will
//!   once a key leaks.

#![no_main]

use core_crypto::{SealKey, open, peek_version};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // Unauthenticated header parsing, on anything at all.
    let _ = peek_version(data);

    let key = SealKey::from_bytes([0x5a; 32]);
    let _ = open(data, &key, None);
    // The anti-rollback branch, which compares before it decrypts.
    let _ = open(data, &key, Some(u64::MAX / 2));

    // And the body parser, reached through a file that really does
    // authenticate: seal the fuzzer's bytes as the JSON body, so `view()` runs
    // on attacker-shaped input that has already passed the tag.
    if let Ok(text) = std::str::from_utf8(data)
        && let Ok(contents) = serde_json::from_str::<core_crypto::Contents>(text)
        && let Ok(sealed) = core_crypto::seal(&contents, &key)
        && let Ok(opened) = open(&sealed, &key, None)
    {
        let _ = opened.view();
    }
});
