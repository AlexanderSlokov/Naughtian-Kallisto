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
//! measurement and never allocates again — the chunk it reads into, and the two
//! lists of addresses it compares. The first version of this did not, and its
//! own multi-megabyte reads landed on the freed blocks it was about to look at
//! — so it reported a clean process whatever it was given. Nothing reachable
//! from `Scanner::scan` may allocate, which is why the region parser hands back
//! one range at a time instead of collecting them.
//!
//! Read the *new addresses*, not the absolute counts. Sealing the fixture at
//! the start parses the secret into owned `serde_json` values, and those
//! allocations keep the secret after they are freed — which is the very thing
//! this milestone removed from the *serving* path, and which is visible here as
//! a baseline that never returns to zero. In production that step runs in the
//! CLI, not in the resolver. That baseline then drifts *down* as later
//! allocations land on those freed blocks and scrub them, so what is checked is
//! where each copy sits, not how many there are.
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

    scanner.record_baseline();
    report("baseline, before the file is opened", &scanner);

    let snapshot = build_snapshot(&sealed, &key);
    scanner.scan();
    assert_no_new_copies("after building a snapshot", &scanner);

    serve_one_response(&snapshot, &mut scanner);

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
/// the few being sent are in the clear. Reported, not asserted on.
fn serve_one_response(snapshot: &Snapshot, scanner: &mut Scanner) {
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

    scanner.scan();
    drop(body);
    report("while a response body is alive", scanner);

    scanner.scan();
    report("after that body is dropped", scanner);
}

/// The one thing this milestone has to prove: building a snapshot leaves no
/// copy of the cleartext at an address the baseline did not already hold.
///
/// Counting cannot say this. Sealing the fixture leaves copies in freed blocks,
/// and the resolver's own allocations land on some of them and scrub them, so
/// on a perfectly clean run the total drifts *down* — which an equality check
/// on the count reports as a failure, and a `<=` check would paper over the
/// case where one copy is scrubbed and one is added. Addresses do not drift: a
/// needle where the baseline never had one came from the step under test.
fn assert_no_new_copies(what: &str, scanner: &Scanner) {
    report(what, scanner);
    let new = scanner.new_copies();
    assert_eq!(
        new, 0,
        "{what}: {new} copy(ies) of the cleartext at an address the baseline did not hold"
    );
}

fn report(what: &str, scanner: &Scanner) {
    let total = scanner.found.len();
    let delta = total as isize - scanner.baseline.len() as isize;
    println!(
        "{what:<38} {total:>2} copy(ies)  ({delta:+} vs baseline, {} at a new address)",
        scanner.new_copies()
    );
}

/// Finds every occurrence of [`NEEDLE`] in this process's anonymous and heap
/// mappings, recording *where* each one sits, without allocating anything after
/// construction.
struct Scanner {
    maps: String,
    chunk: Vec<u8>,
    baseline: Vec<usize>,
    found: Vec<usize>,
}

impl Scanner {
    const CHUNK: usize = 1 << 22; // 4 MiB
    /// Room for far more copies than any run produces, reserved up front so
    /// that recording a hit never allocates. `Sweep::record` panics rather than
    /// grow past it.
    const MAX_COPIES: usize = 4096;

    fn new() -> Self {
        Self {
            maps: String::with_capacity(1 << 20),
            chunk: vec![0u8; Self::CHUNK],
            baseline: Vec::with_capacity(Self::MAX_COPIES),
            found: Vec::with_capacity(Self::MAX_COPIES),
        }
    }

    /// Where the needle already is before the step under test runs. Every later
    /// scan is judged against these addresses, not against their count.
    fn record_baseline(&mut self) {
        self.scan();
        self.baseline.clear();
        self.baseline.extend_from_slice(&self.found);
    }

    /// Copies sitting where the baseline had none. A copy that *disappears*
    /// between scans is allocator reuse, not a finding.
    fn new_copies(&self) -> usize {
        self.found
            .iter()
            .filter(|&&at| !self.baseline.contains(&at))
            .count()
    }

    fn scan(&mut self) {
        self.reload_maps();
        // Split the borrow by hand: the line being parsed lives in `maps` while
        // the sweep writes into `chunk` and `found`.
        let Self {
            maps, chunk, found, ..
        } = self;
        found.clear();
        let mut sweep = Sweep::new(chunk, found);
        for (lo, hi) in maps.lines().filter_map(own_anonymous_range) {
            sweep.read_range(lo, hi);
        }
    }

    fn reload_maps(&mut self) {
        self.maps.clear();
        File::open("/proc/self/maps")
            .expect("this example is Linux-only")
            .read_to_string(&mut self.maps)
            .expect("/proc/self/maps is UTF-8");
    }
}

/// One pass over the address space, holding only what it writes to — so that
/// `Scanner::maps` can stay borrowed by the line being parsed.
struct Sweep<'a> {
    mem: File,
    chunk: &'a mut [u8],
    found: &'a mut Vec<usize>,
}

impl<'a> Sweep<'a> {
    fn new(chunk: &'a mut [u8], found: &'a mut Vec<usize>) -> Self {
        Self {
            mem: File::open("/proc/self/mem").expect("this example is Linux-only"),
            chunk,
            found,
        }
    }

    /// Reads `[lo, hi)` a chunk at a time, recording each needle it holds.
    fn read_range(&mut self, lo: usize, hi: usize) {
        let mut at = lo;
        while at < hi {
            let want = (hi - at).min(self.chunk.len());
            if self.mem.seek(SeekFrom::Start(at as u64)).is_err()
                || self.mem.read_exact(&mut self.chunk[..want]).is_err()
            {
                break; // guard pages and the like
            }
            self.record(at, want);
            if want < self.chunk.len() {
                break;
            }
            // Chunked with an overlap, so a needle straddling a chunk boundary
            // is still seen once: it lands whole in the chunk that follows.
            at += self.chunk.len() - (NEEDLE.len() - 1);
        }
    }

    /// Records the absolute address of every needle in the first `len` bytes of
    /// the chunk, which was read starting at `base`.
    fn record(&mut self, base: usize, len: usize) {
        let Self { chunk, found, .. } = self;
        for (offset, window) in chunk[..len].windows(NEEDLE.len()).enumerate() {
            if window != NEEDLE.as_bytes() {
                continue;
            }
            assert!(
                found.len() < found.capacity(),
                "more than {} copies of the needle in this process; raise Scanner::MAX_COPIES",
                found.capacity()
            );
            found.push(base + offset);
        }
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
