//! Kallisto — a local, read-only secrets resolver that speaks Vault KV-v2.
//!
//! Four modules and nothing else (ADR-0015, ADR-0016):
//!
//! * [`config`] reads the YAML, and refuses a listener that is not loopback.
//! * [`resolver`] polls one encrypted file on an S3-compatible bucket,
//!   authenticates it, refuses anything older than what it holds, and swaps the
//!   result in whole.
//! * [`server`] answers Vault's read surface out of that snapshot and answers
//!   403 to everything that writes.
//! * [`event`] is the thread-per-core worker pool the whole thing runs on.
//!
//! What used to be here — a storage engine, a RocksDB backend, a cuckoo table,
//! a gossip-based control plane, a registry of pluggable engines — is gone.
//! ADR-0015 redefined the problem from "a high-performance secrets server" to
//! "a resolver for one machine's apps", and almost all of that machinery was
//! answering the first question.

pub mod config;
pub mod event;
pub mod resolver;
pub mod server;
