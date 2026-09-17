//! Does a secret's cleartext survive anywhere in this process once the
//! snapshot has been built? (ADR-0015 D13, duck plan M5.)
//!
//! Scans this process's own anonymous and heap mappings, which needs no ptrace
//! permission — a useful property, since `harden_process` makes the real server
//! unreadable even to its own user.
//!
//! An example rather than a test: a whole-process scan also sees stack copies
//! and allocator reuse, so it is a thing to run and read, not a thing to gate a
//! build on. The gating version is
//! `nothing_in_a_live_snapshot_carries_the_cleartext` in `src/resolver/
//! snapshot.rs`, which checks the data structure rather than the address space.
//!
//! The scanner allocates every buffer it will ever need before the first
//! measurement and never allocates again. The first version of this did not,
//! and its own multi-megabyte reads landed on the freed blocks it was about to
//! look at — so it reported a clean process whatever it was given.
//!
//! Read the *deltas*, not the absolute counts. Sealing the fixture at the start
//! parses the secret into owned `serde_json` values, and those allocations keep
//! the secret after they are freed — which is the very thing this milestone
//! removed from the *serving* path, and which is visible here as a baseline
//! that never returns to zero. In production that step runs in the CLI, not in
//! the resolver.
//!
//!     cargo run --example cleartext_scan

use std::io::{Read, Seek, SeekFrom};

use core_crypto::{Contents, SealKey};
use naughtian_kallisto::resolver::Snapshot;
use zeroize::Zeroizing;

const NEEDLE: &str = "correct-horse-battery-staple-9f3a";

/// A realistic secret value rather than a short one. `free()` writes allocator
/// metadata over the first bytes of a released chunk, so a 33-byte string is
/// partly destroyed by being freed — which would make this scan look clean for
/// the wrong reason. The needle sits in the middle of a kilobyte of padding, in
/// the part `free()` leaves alone.
fn padded_secret() -> String {
    format!("{}{NEEDLE}{}", "a".repeat(512), "b".repeat(512))
}

fn main() {
    let mut scanner = Scanner::new();
    let key = SealKey::from_bytes([11u8; 32]);

    // Sealing the fixture is a CLI-side operation, and it parses the secret
    // into owned values. Whatever those leave behind is the baseline; what
    // matters below is what the *resolver* adds to it.
    let sealed = {
        let plain = Zeroizing::new(format!(
            r#"{{"version":1,"secrets":{{"app/db":{{"password":"{}"}}}}}}"#,
            padded_secret()
        ));
        let contents: Contents = serde_json::from_str(&plain).unwrap();
        let bytes = core_crypto::seal(&contents, &key).unwrap();
        drop(contents);
        bytes
    };
    let baseline = scanner.scan();
    report("baseline, before the file is opened", baseline, baseline);

    // The resolver's path: open the file, borrow its plaintext, seal every
    // secret under the barrier, drop the plaintext. This is the number the
    // milestone is about, and it must be zero.
    let snapshot = {
        let opened = core_crypto::open(&sealed, &key, None).unwrap();
        Snapshot::build(opened.view().unwrap(), None).unwrap()
    };
    let built = scanner.scan();
    report("after building a snapshot", built, baseline);
    assert_eq!(
        built, baseline,
        "building a snapshot left cleartext in memory"
    );

    // Serving does put the secret in the clear, in the response body, and
    // dropping that body does not scrub it. ADR-0015 D13 says exactly this:
    // at any moment the few being sent are in the clear.
    let body = snapshot
        .with_secret("app/db", ToString::to_string)
        .unwrap()
        .unwrap();
    assert!(
        body.contains(NEEDLE),
        "the secret should be readable at all"
    );
    let during = scanner.scan();
    drop(body);
    report("while a response body is alive", during, baseline);
    report("after that body is dropped", scanner.scan(), baseline);

    println!(
        "\nthis thread's decryption buffer wiped: {}",
        core_crypto::barrier::scratch_is_wiped()
    );
}

fn report(what: &str, hits: usize, baseline: usize) {
    let delta = hits as isize - baseline as isize;
    println!("{what:<38} {hits:>2} copy(ies)  ({delta:+} vs baseline)");
}

/// Counts occurrences of [`NEEDLE`] in anonymous and heap mappings, without
/// allocating anything after construction.
struct Scanner {
    maps: String,
    chunk: Vec<u8>,
}

impl Scanner {
    const CHUNK: usize = 1 << 22; // 4 MiB

    fn new() -> Self {
        Self {
            maps: String::with_capacity(1 << 20),
            chunk: vec![0u8; Self::CHUNK],
        }
    }

    fn scan(&mut self) -> usize {
        self.maps.clear();
        std::fs::File::open("/proc/self/maps")
            .expect("this example is Linux-only")
            .read_to_string(&mut self.maps)
            .unwrap();

        let mut mem = std::fs::File::open("/proc/self/mem").unwrap();
        let mut hits = 0;

        for line in self.maps.lines() {
            let mut fields = line.split_whitespace();
            let range = fields.next().unwrap();
            let perms = fields.next().unwrap();
            let path = fields.nth(3).unwrap_or("");
            // File-backed mappings hold the string literal itself, which is not
            // a leak.
            if !perms.starts_with('r') || !(path.is_empty() || path == "[heap]") {
                continue;
            }

            let (lo, hi) = range.split_once('-').unwrap();
            let lo = usize::from_str_radix(lo, 16).unwrap();
            let hi = usize::from_str_radix(hi, 16).unwrap();

            // Chunked with an overlap, so a needle straddling a chunk boundary
            // is still counted once.
            let overlap = NEEDLE.len() - 1;
            let mut at = lo;
            while at < hi {
                let want = (hi - at).min(Self::CHUNK);
                if mem.seek(SeekFrom::Start(at as u64)).is_err()
                    || mem.read_exact(&mut self.chunk[..want]).is_err()
                {
                    break; // guard pages and the like
                }
                hits += self.chunk[..want]
                    .windows(NEEDLE.len())
                    .filter(|window| *window == NEEDLE.as_bytes())
                    .count();
                if want < Self::CHUNK {
                    break;
                }
                at += Self::CHUNK - overlap;
            }
        }
        hits
    }
}
