//! Access log, error log, and counters (ADR-0015 D15).
//!
//! **There is no audit log here, and nothing in this crate may be called one.**
//! The distinction is the whole design and D15 spells it out: an audit log
//! records *before* it serves, so a full queue means refusing to serve; this
//! records after the fact and drops when it falls behind. Naming a
//! drop-on-full log "audit" would let somebody believe they can answer "who
//! read the Stripe key" when they cannot, and a limit you have hidden is a
//! trap. `tests/security_invariants.rs` enforces the naming rather than
//! trusting it.
//!
//! What the two logs share, and what D15 requires them to share: every path and
//! every token goes through a keyed hash before it is written, and no secret
//! value is ever written at all. A path leaking through an error line has
//! leaked exactly as badly as one leaking through an access line.

pub mod access_log;
pub mod metrics;
pub mod redact;

use std::io::Write;

pub use access_log::{AccessLog, Line, Producer, Record};
pub use metrics::{Outcome, Status, WorkerMetrics, render};
pub use redact::{Id, LogKey};

/// One error line, on stderr, under the same hygiene as the access log.
///
/// `subject` is a short fixed word for what went wrong; `detail` must already
/// be safe to print. Error types in this workspace are built so that it is —
/// `SealError`, `SourceError` and `TokenError` all carry counts and positions
/// rather than content, and `tests/security_invariants.rs` holds them to it.
///
/// Anything caller-supplied has to be an [`Id`] before it reaches the format
/// string — a path or a token never goes in as text.
pub fn error_line(subject: &str, detail: &std::fmt::Arguments<'_>) {
    let mut stderr = std::io::stderr().lock();
    let _ = writeln!(stderr, "kallisto: {subject}: {detail}");
}

#[macro_export]
macro_rules! error_log {
    ($subject:expr, $($arg:tt)*) => {
        $crate::error_line($subject, &format_args!($($arg)*))
    };
}
