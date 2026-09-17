//! The four steps that are the whole program (ADR-0015 D10, as amended by
//! ADR-0016 QĐ-4):
//!
//! 1. ask the source whether the sealed file changed;
//! 2. if it did, open it, authenticate it, and check it is not older than what
//!    we already serve;
//! 3. valid file, swap the whole table in; invalid file, keep the old table and
//!    say so;
//! 4. the HTTP layer answers out of whatever table is currently in place.
//!
//! Steps 1-3 live here. Step 4 lives in [`crate::server`] and reads the table
//! directly — there is no port between them, on purpose.

pub mod bucket;
pub mod refresh;
pub mod sigv4;
pub mod snapshot;
pub mod source;

pub use bucket::{BucketConfig, BucketError, BucketSource};
pub use refresh::{RefreshError, Refresher};
pub use snapshot::{Snapshot, SnapshotSlot};
pub use source::{DiskSource, Fetched, SecretSource, SourceError};
