//! The poll loop: steps 1 to 3 of ADR-0015 D10.
//!
//! Runs on its own runtime, never on a serving worker (ADR-0016 QĐ-3): a slow
//! bucket must never occupy a core that is answering requests.
//!
//! The loop never exits. A bucket that is down, a file that fails to open, a
//! key that is wrong — none of these are fatal. Kallisto keeps serving whatever
//! table it already holds and keeps trying, because an app that cannot reach
//! its secrets is an outage and a stale secret usually is not.

use std::{path::PathBuf, sync::Arc, time::Duration};

use core_crypto::{SealError, SealKey};
use tokio::sync::Notify;

use super::{
    snapshot::{Snapshot, SnapshotError, SnapshotSlot},
    source::{Fetched, SecretSource, SourceError},
};

pub const DEFAULT_INTERVAL: Duration = Duration::from_secs(30);

#[derive(Debug, thiserror::Error)]
pub enum RefreshError {
    #[error("sealed file refused: {0}")]
    Seal(#[from] SealError),
    /// The file was genuine but describes something unservable — so far, only a
    /// token table that could never authenticate anyone.
    #[error("sealed file refused: {0}")]
    Unservable(#[from] SnapshotError),
}

/// What one poll did. Returned rather than logged in place so the loop is
/// testable without a clock, and so the caller owns the log vocabulary.
#[derive(Debug)]
pub enum Tick {
    /// The source says nothing changed, or it changed back to a version we
    /// already hold.
    Unchanged,
    Loaded {
        version: u64,
        /// False means the encrypted copy could not be written to local disk,
        /// so this machine has lost its cold-start fallback (ADR-0015 D5) even
        /// though it is serving correctly right now.
        cached: bool,
    },
    /// The file arrived and was rejected. The previous table is still in place.
    Rejected(RefreshError),
    SourceDown(SourceError),
}

pub struct Refresher {
    source: Arc<dyn SecretSource>,
    key: SealKey,
    slot: Arc<SnapshotSlot>,
    cache_path: Option<PathBuf>,
    interval: Duration,
    wake: Arc<Notify>,
    etag: Option<String>,
}

impl Refresher {
    pub fn new(
        source: Arc<dyn SecretSource>,
        key: SealKey,
        slot: Arc<SnapshotSlot>,
        cache_path: Option<PathBuf>,
    ) -> Self {
        Self {
            source,
            key,
            slot,
            cache_path,
            interval: DEFAULT_INTERVAL,
            wake: Arc::new(Notify::new()),
            etag: None,
        }
    }

    pub fn every(mut self, interval: Duration) -> Self {
        self.interval = interval;
        self
    }

    /// Hand this to whoever can demand an immediate re-read — the admin
    /// endpoint, and later Sovereign telling the flock to reload now instead of
    /// waiting out the TTL (ADR-0015 D16).
    pub fn waker(&self) -> Arc<Notify> {
        Arc::clone(&self.wake)
    }

    /// Cold start from the encrypted copy on local disk (ADR-0015 D5).
    ///
    /// Called before the first bucket poll so that the version it finds becomes
    /// the floor for the anti-rollback check. Without this, a restart is the
    /// one moment an attacker could feed us an old bucket file unopposed.
    pub async fn warm_from_cache(&mut self) -> Tick {
        let Some(path) = self.cache_path.clone() else {
            return Tick::SourceDown(SourceError::Absent);
        };
        match tokio::fs::read(&path).await {
            Ok(bytes) => self.adopt(bytes, None, None, false).await,
            Err(_) => Tick::SourceDown(SourceError::Absent),
        }
    }

    pub async fn poll_once(&mut self) -> Tick {
        let held = self.slot.version();
        match self.source.fetch(self.etag.as_deref()).await {
            Fetched::NotModified => Tick::Unchanged,
            Fetched::Unavailable(e) => Tick::SourceDown(e),
            Fetched::Body { bytes, etag } => self.adopt(bytes, etag, held, true).await,
        }
    }

    pub async fn run(mut self) {
        let wake = Arc::clone(&self.wake);
        let mut ticker = tokio::time::interval(self.interval);
        loop {
            tokio::select! {
                _ = ticker.tick() => {}
                () = wake.notified() => {}
            }
            let _ = self.poll_once().await;
        }
    }

    async fn adopt(
        &mut self,
        bytes: Vec<u8>,
        etag: Option<String>,
        held: Option<u64>,
        write_cache: bool,
    ) -> Tick {
        let offered = match core_crypto::peek_version(&bytes) {
            Ok(v) => v,
            Err(e) => return Tick::Rejected(e.into()),
        };

        // Content versions are monotonic and never reused, so the same number
        // means the same file. Skipping here is not just an optimisation: it
        // means a same-numbered forgery can never displace the good table we
        // already serve, because we do not even look at it.
        if Some(offered) == held {
            self.etag = etag;
            return Tick::Unchanged;
        }

        let opened = match core_crypto::open(&bytes, &self.key, held) {
            Ok(opened) => opened,
            // Forged, rolled back, or sealed with a different key. Keep the
            // table we have — ADR-0015 D10 step 3.
            Err(e) => return Tick::Rejected(e.into()),
        };

        let version = opened.content_version();
        // `view` borrows the cleartext, which `opened` wipes at the end of this
        // block. The snapshot it builds owns only sealed bytes (ADR-0015 D13),
        // so nothing readable survives past here.
        let snapshot = match opened
            .view()
            .map_err(Into::into)
            .and_then(|view| Snapshot::build(view, etag.clone()).map_err(RefreshError::from))
        {
            Ok(snapshot) => snapshot,
            Err(e) => return Tick::Rejected(e),
        };
        for name in snapshot.unknown_policies() {
            // Not fatal — an undefined policy grants nothing — but the operator
            // has a typo they cannot see any other way.
            eprintln!(
                "kallisto: a token refers to policy {name:?}, which the file does not define"
            );
        }
        self.slot.store(snapshot);
        self.etag = etag;

        let cached = if write_cache {
            self.write_cache(&bytes).await
        } else {
            true
        };
        Tick::Loaded { version, cached }
    }

    /// Stores the file **still encrypted**. ADR-0001, as amended by ADR-0015
    /// D5: the decrypted form never touches disk, but the sealed form must, or
    /// a VM that reboots while the bucket is down has nothing to start from.
    async fn write_cache(&self, bytes: &[u8]) -> bool {
        let Some(path) = &self.cache_path else {
            return true;
        };
        let temp = path.with_extension("tmp");
        // Write-then-rename, so a crash mid-write cannot leave a truncated file
        // where the cold-start path expects a whole one.
        tokio::fs::write(&temp, bytes).await.is_ok() && tokio::fs::rename(&temp, path).await.is_ok()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use core_crypto::{Contents, seal};

    use super::*;
    use crate::resolver::source::DiskSource;

    fn key() -> SealKey {
        SealKey::from_bytes([4u8; 32])
    }

    fn contents(version: u64, secret: &str) -> Contents {
        Contents {
            version,
            secrets: BTreeMap::from([(
                "app/db".to_string(),
                serde_json::json!({ "pass": secret }),
            )]),
            policies: BTreeMap::new(),
            tokens: BTreeMap::new(),
            token_key: None,
        }
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join("kallisto-refresh-tests");
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    fn refresher(file: &PathBuf, cache: Option<PathBuf>) -> (Refresher, Arc<SnapshotSlot>) {
        let slot = Arc::new(SnapshotSlot::empty());
        let source = Arc::new(DiskSource::new(file));
        (
            Refresher::new(source, key(), Arc::clone(&slot), cache),
            slot,
        )
    }

    #[tokio::test]
    async fn a_good_file_is_adopted_and_cached_encrypted() {
        let file = scratch("good.kal");
        let cache = scratch("good.cache");
        std::fs::write(&file, seal(&contents(3, "first"), &key()).unwrap()).unwrap();

        let (mut r, slot) = refresher(&file, Some(cache.clone()));
        assert!(matches!(
            r.poll_once().await,
            Tick::Loaded {
                version: 3,
                cached: true
            }
        ));
        assert_eq!(slot.version(), Some(3));

        // The cached copy must still be sealed. This is ADR-0001's invariant and
        // the cheapest possible test of it.
        let on_disk = std::fs::read(&cache).unwrap();
        assert_eq!(&on_disk[..8], b"KALLISTO");
        assert!(!on_disk.windows(5).any(|w| w == b"first"));
    }

    #[tokio::test]
    async fn the_same_version_is_skipped_without_being_opened() {
        let file = scratch("same.kal");
        std::fs::write(&file, seal(&contents(3, "first"), &key()).unwrap()).unwrap();
        let (mut r, _slot) = refresher(&file, None);
        assert!(matches!(
            r.poll_once().await,
            Tick::Loaded { version: 3, .. }
        ));

        // Re-seal the *same version* with different content: a forgery that got
        // the counter right cannot displace what we serve.
        std::fs::write(&file, seal(&contents(3, "second"), &key()).unwrap()).unwrap();
        assert!(matches!(r.poll_once().await, Tick::Unchanged));
    }

    #[tokio::test]
    async fn a_rolled_back_file_leaves_the_previous_table_in_place() {
        let file = scratch("rollback.kal");
        std::fs::write(&file, seal(&contents(7, "new"), &key()).unwrap()).unwrap();
        let (mut r, slot) = refresher(&file, None);
        assert!(matches!(
            r.poll_once().await,
            Tick::Loaded { version: 7, .. }
        ));

        std::fs::write(&file, seal(&contents(5, "old"), &key()).unwrap()).unwrap();
        let tick = r.poll_once().await;
        assert!(
            matches!(
                tick,
                Tick::Rejected(RefreshError::Seal(SealError::Rollback {
                    held: 7,
                    offered: 5
                }))
            ),
            "got {tick:?}"
        );
        assert_eq!(slot.version(), Some(7), "the good table must survive");
        assert_eq!(
            slot.load()
                .unwrap()
                .with_secret("app/db", ToString::to_string)
                .map(Result::unwrap)
                .as_deref(),
            Some(r#"{"pass":"new"}"#)
        );
    }

    #[tokio::test]
    async fn a_forged_file_leaves_the_previous_table_in_place() {
        let file = scratch("forged.kal");
        std::fs::write(&file, seal(&contents(1, "good"), &key()).unwrap()).unwrap();
        let (mut r, slot) = refresher(&file, None);
        assert!(matches!(
            r.poll_once().await,
            Tick::Loaded { version: 1, .. }
        ));

        let mut forged = seal(&contents(2, "evil"), &SealKey::from_bytes([9u8; 32])).unwrap();
        forged[HEADER_TAMPER_TARGET] ^= 0x01;
        std::fs::write(&file, &forged).unwrap();

        assert!(matches!(
            r.poll_once().await,
            Tick::Rejected(RefreshError::Seal(SealError::AuthFailed))
        ));
        assert_eq!(slot.version(), Some(1));
    }

    const HEADER_TAMPER_TARGET: usize = 40;

    #[tokio::test]
    async fn a_missing_source_is_reported_not_fatal() {
        let (mut r, slot) = refresher(&scratch("absent.kal.nope"), None);
        assert!(matches!(
            r.poll_once().await,
            Tick::SourceDown(SourceError::Absent)
        ));
        assert_eq!(slot.version(), None, "nothing loaded means stay sealed");
    }

    /// ADR-0015 D5: a reboot while the bucket is down still has to produce a
    /// serving Kallisto, and the version it recovers becomes the rollback
    /// floor.
    #[tokio::test]
    async fn cold_start_reads_the_encrypted_copy_from_disk() {
        let cache = scratch("cold.cache");
        std::fs::write(&cache, seal(&contents(11, "cold"), &key()).unwrap()).unwrap();

        let (mut r, slot) = refresher(&scratch("no-bucket.kal"), Some(cache));
        assert!(matches!(
            r.warm_from_cache().await,
            Tick::Loaded { version: 11, .. }
        ));
        assert_eq!(slot.version(), Some(11));

        // And now the bucket being unreachable changes nothing about serving.
        assert!(matches!(r.poll_once().await, Tick::SourceDown(_)));
        assert_eq!(slot.version(), Some(11));
    }
}
