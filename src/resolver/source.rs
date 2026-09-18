//! Where the sealed file comes from.
//!
//! This is the one place ADR-0016 QĐ-4 keeps a real port. It earns it: a third
//! source is plausible, and this code runs twice a minute, so the cost of
//! dynamic dispatch here is nothing. Everything on the *serving* side is welded
//! straight into the core instead.
//!
//! The trait is desugared by hand rather than using `async_trait`. `async fn`
//! in a trait is not dyn-compatible yet, and one boxed future every thirty
//! seconds is cheaper than another proc-macro dependency.

use std::{future::Future, path::PathBuf, pin::Pin};

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

#[derive(Debug, thiserror::Error)]
pub enum SourceError {
    #[error("source is unreachable: {0}")]
    Unreachable(String),
    #[error("source answered {status}")]
    Status { status: u16 },
    /// The bucket is telling us to back off. ADR-0015 D14: Kallisto *receives*
    /// this dialect, it never speaks it — the app gets Vault's 429 instead.
    #[error("source asked us to slow down")]
    SlowDown,
    #[error("no sealed file at the configured location")]
    Absent,
}

/// What a poll turned up. `NotModified` is the common case by a wide margin,
/// which is why conditional GET is mandatory (ADR-0015 D2).
#[derive(Debug)]
pub enum Fetched {
    NotModified,
    Body {
        bytes: Vec<u8>,
        etag: Option<String>,
    },
    Unavailable(SourceError),
}

pub trait SecretSource: Send + Sync {
    /// `etag` is whatever the last successful fetch returned, so the source can
    /// answer `NotModified` without shipping the bytes again.
    fn fetch<'a>(&'a self, etag: Option<&'a str>) -> BoxFuture<'a, Fetched>;

    /// For log lines and the health endpoint. Must never include credentials.
    fn describe(&self) -> String;
}

/// Reads the sealed file from the local filesystem.
///
/// Two jobs, both real: it is the source used by tests, and it is the
/// cold-start fallback of ADR-0015 D5 — when the bucket is down, the encrypted
/// copy on this VM's own disk is what lets the payment app start at all.
pub struct DiskSource {
    path: PathBuf,
}

impl DiskSource {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }
}

impl SecretSource for DiskSource {
    fn fetch<'a>(&'a self, _etag: Option<&'a str>) -> BoxFuture<'a, Fetched> {
        Box::pin(async move {
            match tokio::fs::read(&self.path).await {
                Ok(bytes) => Fetched::Body { bytes, etag: None },
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    Fetched::Unavailable(SourceError::Absent)
                }
                Err(e) => Fetched::Unavailable(SourceError::Unreachable(e.kind().to_string())),
            }
        })
    }

    fn describe(&self) -> String {
        format!("file://{}", self.path.display())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn disk_source_reports_a_missing_file_as_absent_not_as_an_io_error() {
        // The distinction matters to the caller: `Absent` on first boot means
        // "nothing cached yet, stay sealed and keep polling", while a real I/O
        // error means the disk itself is a problem worth logging loudly.
        let source = DiskSource::new("/nonexistent/kallisto/secrets.kal");
        assert!(matches!(
            source.fetch(None).await,
            Fetched::Unavailable(SourceError::Absent)
        ));
    }

    #[tokio::test]
    async fn disk_source_returns_the_bytes_it_was_given() {
        let dir = std::env::temp_dir().join("kallisto-disk-source-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("secrets.kal");
        std::fs::write(&path, b"sealed-bytes").unwrap();

        let source = DiskSource::new(&path);
        match source.fetch(None).await {
            Fetched::Body { bytes, etag } => {
                assert_eq!(bytes, b"sealed-bytes");
                assert!(etag.is_none(), "a plain file has no entity tag");
            }
            other => panic!("expected a body, got {other:?}"),
        }
        std::fs::remove_file(&path).unwrap();
    }

    #[tokio::test]
    async fn describe_names_the_location_without_leaking_anything() {
        let source = DiskSource::new("/var/lib/kallisto/secrets.kal");
        assert_eq!(source.describe(), "file:///var/lib/kallisto/secrets.kal");
    }
}
