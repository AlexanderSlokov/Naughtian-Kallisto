//! The encryption barrier, storage-facing half (ADR-0015 D13).
//!
//! Kallisto never trusts the place its secret file is kept. Everything that
//! goes out to the bucket is sealed first and everything that comes back is
//! opened and authenticated before a single byte of it is believed. This crate
//! owns that boundary and nothing else: it knows the file format, the key, and
//! the freshness rule, and it knows nothing about HTTP, buckets, or policy.

pub mod hex;
pub mod key;
pub mod sealed_file;

pub use key::{KEY_LEN, KeyError, SealKey};
pub use sealed_file::{Contents, PolicyRule, SealError, open, peek_version, seal};
