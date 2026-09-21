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
//! look at — so it reported a clean process whatever it was given. Nothing
//! reachable from `Scanner::scan` may allocate, which is why the region parser
//! hands back one range at a time instead of collecting them.
//!
//! Read the *deltas*, not the absolute counts. Sealing the fixture at the start
//! parses the secret into owned `serde_json` values, and those allocations keep
//! the secret after they are freed — which is the very thing this milestone
//! removed from the *serving* path, and which is visible here as a baseline
//! that never returns to zero. In production that step runs in the CLI, not in
//! the resolver.
//!
//!     cargo run --example cleartext_scan

use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
};

use core_crypto::{Contents, SealKey};
use naughtian_kallisto::resolver::Snapshot;
use zeroize::Zeroizing;

const NEEDLE: &str = "duck-fixture-value-9f3a";

fn main() {
    let mut scanner = Scanner::new();
    let key = SealKey::from_bytes([11u8; 32]);
    let sealed = seal_fixture(&key);

    let baseline = scanner.scan();
    report("baseline, before the file is opened", baseline, baseline);

    let snapshot = build_snapshot(&sealed, &key);
    assert_matches_baseline("after building a snapshot", scanner.scan(), baseline);

    serve_one_response(&snapshot, &mut scanner, baseline);

    println!(
        "\nthis thread's decryption buffer wiped: {}",
        core_crypto::barrier::scratch_is_wiped()
    );
}

/// A realistic secret value rather than a short one. `free()` writes allocator
/// metadata over the first bytes of a released chunk, so a 33-byte string is
/// partly destroyed by being freed — which would make this scan look clean for
/// the wrong reason. The needle sits in the middle of a kilobyte of padding, in
/// the part `free()` leaves alone.
fn padded_secret() -> String {
    format!("{}{NEEDLE}{}", "a".repeat(512), "b".repeat(512))
}

/// Sealing the fixture is a CLI-side operation, and it parses the secret into
/// owned values. Whatever those leave behind is the baseline; what matters
/// afterwards is what the *resolver* adds to it.
fn seal_fixture(key: &SealKey) -> Vec<u8> {
    let plain = Zeroizing::new(format!(
        r#"{{"version":1,"secrets":{{"app/db":{{"password":"{}"}}}}}}"#,
        padded_secret()
    ));
    let contents: Contents = serde_json::from_str(&plain).expect("the fixture is valid Contents");
    let sealed = core_crypto::seal(&contents, key).expect("sealing the fixture");
    drop(contents);
    sealed
}

/// The resolver's path: open the file, borrow its plaintext, seal every secret
/// under the barrier, drop the plaintext.
fn build_snapshot(sealed: &[u8], key: &SealKey) -> Snapshot {
    let opened = core_crypto::open(sealed, key, None).expect("opening what we just sealed");
    Snapshot::build(opened.view().unwrap(), None).expect("building from a valid view")
}

/// Serving does put the secret in the clear, in the response body, and dropping
/// that body does not scrub it. ADR-0015 D13 says exactly this: at any moment
/// the few being sent are in the clear.
fn serve_one_response(snapshot: &Snapshot, scanner: &mut Scanner, baseline: usize) {
    let body = snapshot
        .with_secret("app/db", ToString::to_string)
        .expect("the fixture holds app/db")
        .expect("the barrier refused its own ciphertext");
    // The body itself stays out of this message: it is the very cleartext the
    // scan below is hunting for, and a failure must not print it.
    assert!(
        body.contains(NEEDLE),
        "app/db came back without the needle — the fixture or the barrier is wrong"
    );

    let during = scanner.scan();
    drop(body);
    report("while a response body is alive", during, baseline);
    report("after that body is dropped", scanner.scan(), baseline);
}

/// The one line this milestone has to prove: building a snapshot adds no copy
/// of its own on top of what sealing the fixture already left behind.
///
/// Equality, not `<=`, so that a *drop* in the count is also surfaced: it means
/// the resolver's own allocations landed on a freed baseline copy and scrubbed
/// it, which makes the baseline stop being a baseline. That is what the check
/// currently reports on this machine — see the note in the duck plan.
fn assert_matches_baseline(what: &str, hits: usize, baseline: usize) {
    report(what, hits, baseline);
    assert_eq!(
        hits, baseline,
        "{what}: found {hits} copy(ies) of the cleartext, expected the baseline {baseline}"
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
        self.reload_maps();
        // Split the borrow by hand: the line being parsed lives in `maps` while
        // `chunk` is being written into.
        let Self { maps, chunk } = self;
        let mut mem = File::open("/proc/self/mem").expect("this example is Linux-only");
        maps.lines()
            .filter_map(own_anonymous_range)
            .map(|(lo, hi)| count_in_range(&mut mem, chunk, lo, hi))
            .sum()
    }

    fn reload_maps(&mut self) {
        self.maps.clear();
        File::open("/proc/self/maps")
            .expect("this example is Linux-only")
            .read_to_string(&mut self.maps)
            .expect("/proc/self/maps is UTF-8");
    }
}

/// The mappings worth reading, as a `[lo, hi)` pair: this process's own
/// readable anonymous memory, plus its heap.
///
/// Takes one line and returns one range rather than collecting them — see the
/// note on allocation at the top of this file.
fn own_anonymous_range(line: &str) -> Option<(usize, usize)> {
    // address perms offset dev inode pathname
    let mut fields = line.split_whitespace();
    let range = fields.next()?;
    let perms = fields.next()?;
    let pathname = fields.nth(3).unwrap_or("");

    if !perms.starts_with('r') {
        return None;
    }
    // File-backed mappings hold the string literal itself, which is not a leak.
    if !pathname.is_empty() && pathname != "[heap]" {
        return None;
    }

    let (lo, hi) = range.split_once('-')?;
    Some((hex_address(lo, line), hex_address(hi, line)))
}

fn hex_address(field: &str, line: &str) -> usize {
    usize::from_str_radix(field, 16).unwrap_or_else(|why| {
        panic!("/proc/self/maps: expected a hex address, got {field:?} in {line:?} ({why})")
    })
}

/// Reads `[lo, hi)` a chunk at a time, counting the needle in each.
fn count_in_range(mem: &mut File, chunk: &mut [u8], lo: usize, hi: usize) -> usize {
    let mut hits: usize = 0;
    let mut at = lo;
    while at < hi {
        let want = (hi - at).min(chunk.len());
        if mem.seek(SeekFrom::Start(at as u64)).is_err()
            || mem.read_exact(&mut chunk[..want]).is_err()
        {
            break; // guard pages and the like
        }
        hits += count_needles(&chunk[..want]);
        if want < chunk.len() {
            break;
        }
        // Chunked with an overlap, so a needle straddling a chunk boundary is
        // still counted once: it lands whole in the chunk that follows.
        at += chunk.len() - (NEEDLE.len() - 1);
    }
    hits
}

fn count_needles(bytes: &[u8]) -> usize {
    bytes
        .windows(NEEDLE.len())
        .filter(|window| *window == NEEDLE.as_bytes())
        .count()
}
