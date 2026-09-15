//! Re-export of the queue, which lives in `components/kallisto_queue`.
//!
//! It was moved out of this crate so `loom` can model-check it: `--cfg loom`
//! applies to the whole dependency graph, and tokio drops `tokio::net` under
//! that cfg, which breaks hyper-util. See ADR-0013 B1/B2 and the `loom` target
//! in the Makefile.
pub use kallisto_queue::{LockFreeQueue, QueueError};
