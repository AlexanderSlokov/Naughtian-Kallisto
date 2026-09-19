//! The RAM half of the encryption barrier (ADR-0015 D13).
//!
//! The file is opened once, in the refresh loop. Each secret is immediately
//! re-sealed under a key that exists only in this process's memory and only
//! for the life of one snapshot, and the file's plaintext buffer wipes itself
//! on the way out. A request then opens exactly the one secret it asked for,
//! into a buffer belonging to that worker thread, and wipes it again before
//! the next request.
//!
//! # What this is worth, stated plainly
//!
//! ADR-0015 D13 is explicit that this is not a wall against root: an attacker
//! who can read process memory can read the barrier key sitting in it. What it
//! removes is the *accidental* disclosure — a core dump, a swapped-out page, a
//! log line that printed a whole table — by making the answer to "what is in
//! this process's memory right now" be "a few dozen ciphertexts, and whichever
//! one or two secrets are mid-flight".
//!
//! And mid-flight is real: the HTTP response body holds a secret in the clear
//! from the moment it is built until the socket has taken it, and nothing here
//! changes that. D13 says as much — "at any moment only the few being sent are
//! in the clear" — and it is the reason this is called a barrier and not
//! protection.
//!
//! # Why a counter nonce
//!
//! AES-GCM needs a nonce never repeated under one key. The key here is fresh
//! random bytes per snapshot, never written anywhere and never reused across
//! files, so a monotonic counter satisfies that by construction and does so
//! without a call to the system RNG per secret. A random nonce would be no
//! safer and could fail.

use std::{
    cell::RefCell,
    sync::atomic::{AtomicU64, Ordering},
};

use aws_lc_rs::aead::{AES_256_GCM, Aad, LessSafeKey, NONCE_LEN, Nonce, UnboundKey};
use zeroize::Zeroize;

use crate::{
    hardening::{self, LockedPages},
    key::SealKey,
    sealed_file::SealError,
};

/// One secret, re-sealed. Nothing here is readable without the barrier that
/// produced it, and that barrier dies with the snapshot.
pub struct Sealed {
    nonce: [u8; NONCE_LEN],
    bytes: Vec<u8>,
}

impl Sealed {
    /// For the test that walks a live snapshot looking for plaintext.
    pub fn ciphertext(&self) -> &[u8] {
        &self.bytes
    }
}

/// Never the contents, and never a length that would hint at them beyond what
/// the ciphertext length already does.
impl std::fmt::Debug for Sealed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Sealed(<ENCRYPTED>)")
    }
}

pub struct Barrier {
    key: LessSafeKey,
    nonce: AtomicU64,
    /// Kept alive so the pages holding the expanded key stay resident. Dropped
    /// with the barrier, which unlocks them.
    _locked: Option<LockedPages>,
}

impl Barrier {
    /// A new barrier under a fresh random key.
    ///
    /// Returns a `Box` because the pages holding the key are locked into RAM by
    /// address, and an address is only worth locking once it has stopped
    /// moving. Boxing first, locking second, is what makes that true.
    pub fn new() -> Result<Box<Self>, SealError> {
        let seed = SealKey::random().map_err(|_| SealError::RandomUnavailable)?;
        let unbound = UnboundKey::new(&AES_256_GCM, seed.expose())
            .map_err(|_| SealError::RandomUnavailable)?;
        // `seed` is dropped — and zeroed — at the end of this function. From
        // here on the only copy of the key is the expanded schedule inside
        // `LessSafeKey`, which is what gets locked below.
        let mut barrier = Box::new(Self {
            key: LessSafeKey::new(unbound),
            nonce: AtomicU64::new(0),
            _locked: None,
        });

        let addr = std::ptr::from_ref::<Self>(&*barrier) as usize;
        barrier._locked = hardening::lock_pages(addr, size_of::<Self>());
        Ok(barrier)
    }

    pub fn seal(&self, plaintext: &[u8]) -> Result<Sealed, SealError> {
        let counter = self.nonce.fetch_add(1, Ordering::Relaxed);
        let mut nonce = [0u8; NONCE_LEN];
        nonce[..8].copy_from_slice(&counter.to_le_bytes());

        let mut bytes = Vec::with_capacity(plaintext.len() + AES_256_GCM.tag_len());
        bytes.extend_from_slice(plaintext);
        self.key
            .seal_in_place_append_tag(
                Nonce::assume_unique_for_key(nonce),
                Aad::empty(),
                &mut bytes,
            )
            .map_err(|_| SealError::AuthFailed)?;

        Ok(Sealed { nonce, bytes })
    }

    /// Opens one secret into this thread's buffer, hands it to `f`, and wipes
    /// the buffer before returning — whether `f` returned or panicked is not
    /// covered, and a panic here would abort the request anyway.
    ///
    /// The closure shape is the point: the plaintext cannot outlive the call,
    /// so there is nowhere for a caller to accidentally keep one.
    pub fn with_plaintext<R>(
        &self,
        sealed: &Sealed,
        f: impl FnOnce(&str) -> R,
    ) -> Result<R, SealError> {
        SCRATCH.with(|scratch| {
            let mut buffer = scratch.borrow_mut();
            let needed = sealed.bytes.len();

            // Grows, never shrinks, and only ever grows from a state that was
            // already wiped — so the block a reallocation leaves behind holds
            // zeroes, not the last secret served.
            if buffer.len() < needed {
                buffer.resize(needed.max(MINIMUM_SCRATCH), 0);
            }
            buffer[..needed].copy_from_slice(&sealed.bytes);

            let outcome = self
                .key
                .open_in_place(
                    Nonce::assume_unique_for_key(sealed.nonce),
                    Aad::empty(),
                    &mut buffer[..needed],
                )
                .map_err(|_| SealError::AuthFailed)
                .and_then(|plain| std::str::from_utf8(plain).map_err(|_| SealError::MalformedBody))
                .map(f);

            buffer[..needed].zeroize();
            outcome
        })
    }
}

/// Big enough that a typical secret never causes a second allocation, small
/// enough that a few dozen worker threads cost nothing worth measuring.
const MINIMUM_SCRATCH: usize = 1024;

thread_local! {
    /// One decryption buffer per worker (ADR-0016 QĐ-3). Not shared, not
    /// locked, and not allocated per request — the read path is hot, and the
    /// alternative is a `malloc` and a `free` on every secret served.
    static SCRATCH: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
}

/// Whether this thread's decryption buffer currently holds only zeroes.
///
/// Returns a bool and never the contents, so it is safe to ship rather than
/// hiding behind `cfg(test)` — which would not be visible to the integration
/// tests that need to assert this in the first place.
///
/// Answers `false` while the buffer is borrowed, which is the honest answer:
/// being borrowed means a secret is in it right now. That is also what lets
/// this be called from inside [`Barrier::with_plaintext`]'s closure without
/// panicking on the `RefCell`.
pub fn scratch_is_wiped() -> bool {
    SCRATCH.with(|scratch| {
        scratch
            .try_borrow()
            .is_ok_and(|buffer| buffer.iter().all(|byte| *byte == 0))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &str = r#"{"username":"payment","password":"duck-fixture-not-a-credential"}"#;

    #[test]
    fn a_secret_round_trips_through_the_barrier() {
        let barrier = Barrier::new().unwrap();
        let sealed = barrier.seal(SECRET.as_bytes()).unwrap();

        let seen = barrier
            .with_plaintext(&sealed, |text| text.to_string())
            .unwrap();
        assert_eq!(seen, SECRET);
    }

    #[test]
    fn the_sealed_form_contains_none_of_the_plaintext() {
        let barrier = Barrier::new().unwrap();
        let sealed = barrier.seal(SECRET.as_bytes()).unwrap();

        let haystack = sealed.ciphertext();
        assert!(
            !contains(haystack, b"duck-fixture-not-a-credential"),
            "the ciphertext carries the secret"
        );
        assert!(
            !contains(haystack, b"payment"),
            "the ciphertext carries a field value"
        );
        assert!(!format!("{sealed:?}").contains("duck-fixture-not-a-credential"));
    }

    /// The property the whole design rests on: after the response is built, the
    /// worker's buffer holds nothing.
    #[test]
    fn the_buffer_is_wiped_after_every_use() {
        let barrier = Barrier::new().unwrap();
        let sealed = barrier.seal(SECRET.as_bytes()).unwrap();

        let saw_it = barrier
            .with_plaintext(&sealed, |text| {
                // Mid-call, the plaintext is genuinely there — otherwise the
                // assertion afterwards would prove nothing.
                assert!(text.contains("duck-fixture-not-a-credential"));
                !scratch_is_wiped()
            })
            .unwrap();
        assert!(saw_it, "the buffer was empty during the call");
        assert!(scratch_is_wiped(), "the buffer kept the secret");
    }

    /// A failed open must wipe too, or a corrupted entry leaves whatever was
    /// copied in sitting there.
    #[test]
    fn the_buffer_is_wiped_even_when_the_open_fails() {
        let barrier = Barrier::new().unwrap();
        let mut sealed = barrier.seal(SECRET.as_bytes()).unwrap();
        sealed.bytes[0] ^= 0xff;

        assert!(barrier.with_plaintext(&sealed, |_| ()).is_err());
        assert!(scratch_is_wiped());
    }

    #[test]
    fn a_secret_sealed_by_one_barrier_cannot_be_opened_by_another() {
        let first = Barrier::new().unwrap();
        let second = Barrier::new().unwrap();
        let sealed = first.seal(SECRET.as_bytes()).unwrap();

        assert!(second.with_plaintext(&sealed, |_| ()).is_err());
    }

    /// Two secrets under one barrier must not share a nonce.
    #[test]
    fn every_seal_uses_a_new_nonce() {
        let barrier = Barrier::new().unwrap();
        let a = barrier.seal(b"same").unwrap();
        let b = barrier.seal(b"same").unwrap();

        assert_ne!(a.nonce, b.nonce);
        assert_ne!(
            a.ciphertext(),
            b.ciphertext(),
            "identical plaintexts produced identical ciphertext"
        );
    }

    #[test]
    fn tampering_with_the_ciphertext_is_caught() {
        let barrier = Barrier::new().unwrap();
        let sealed = barrier.seal(SECRET.as_bytes()).unwrap();

        for index in 0..sealed.bytes.len() {
            let mut damaged = Sealed {
                nonce: sealed.nonce,
                bytes: sealed.bytes.clone(),
            };
            damaged.bytes[index] ^= 0x01;
            assert!(
                barrier.with_plaintext(&damaged, |_| ()).is_err(),
                "byte {index} could be flipped without being noticed"
            );
        }
    }

    /// A secret larger than the starting buffer must still round-trip, and must
    /// still leave the (now larger) buffer wiped.
    #[test]
    fn a_large_secret_grows_the_buffer_and_still_wipes_it() {
        let barrier = Barrier::new().unwrap();
        let big = format!(r#"{{"blob":"{}"}}"#, "x".repeat(8192));
        let sealed = barrier.seal(big.as_bytes()).unwrap();

        let length = barrier.with_plaintext(&sealed, str::len).unwrap();
        assert_eq!(length, big.len());
        assert!(scratch_is_wiped());
    }

    fn contains(haystack: &[u8], needle: &[u8]) -> bool {
        haystack.windows(needle.len()).any(|w| w == needle)
    }
}
