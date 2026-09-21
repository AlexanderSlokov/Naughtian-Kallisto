//! `kallisto-server` — a local, read-only secrets resolver that speaks Vault.
//!
//! The whole program, in order: read the configuration, take the seal key from
//! the environment, start one thread that polls the sealed file, start the
//! serving workers, and get out of the way. Everything interesting is in
//! [`naughtian_kallisto::resolver`] and [`naughtian_kallisto::server`].

use std::{process::ExitCode, sync::Arc, thread};

use naughtian_kallisto::{
    config::{self, ACCESS_KEY_ENV, Config, ConfigError, SEAL_KEY_ENV, SECRET_KEY_ENV, Source},
    event::worker::WorkerPool,
    resolver::{
        BucketConfig, BucketSource, DiskSource, Refresher, SecretSource, SnapshotSlot,
        refresh::Tick,
    },
    server::vault_api::{self, Resolver, Telemetry},
};
use telemetry::error_log;

#[cfg(not(target_env = "msvc"))]
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        // `--help` arrives here too, which is why usage goes to stdout and
        // everything else to stderr.
        Err(Startup::Usage(message)) => {
            println!("{message}");
            ExitCode::from(2)
        }
        Err(Startup::Failed(message)) => {
            eprintln!("kallisto: {message}");
            ExitCode::FAILURE
        }
    }
}

enum Startup {
    Usage(String),
    Failed(String),
}

impl From<ConfigError> for Startup {
    fn from(e: ConfigError) -> Self {
        match e {
            ConfigError::Usage(message) => Startup::Usage(message),
            other => Startup::Failed(other.to_string()),
        }
    }
}

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

fn run() -> Result<(), Startup> {
    // Before anything has read a secret: no core file, and no ptrace. ADR-0015
    // D13's three small things — the third, locking the barrier key into RAM,
    // happens where the key is made.
    let hardening = core_crypto::harden_process();
    if !hardening.complete() {
        error_log!(
            "hardening",
            "incomplete ({hardening:?}); a crash here may write memory to a core file"
        );
    }

    let cli = config::parse_args(std::env::args().skip(1))?;
    let environment = config::from_env(env)?;

    let Some(path) = cli
        .config_path
        .clone()
        .or_else(|| environment.config_path.clone())
    else {
        return Err(ConfigError::Missing.into());
    };
    let file = config::read_file(&path)?;
    let cfg = Config::resolve(file, environment, cli)?;

    // The key never appears in the configuration file, and never in a flag:
    // command-line arguments are world-readable through `ps`.
    let key = env(SEAL_KEY_ENV)
        .ok_or_else(|| {
            Startup::Failed(format!(
                "{SEAL_KEY_ENV} is not set. It holds the 32-byte seal key, hex-encoded."
            ))
        })
        .and_then(|hex| {
            core_crypto::SealKey::from_hex(&hex)
                .map_err(|e| Startup::Failed(format!("{SEAL_KEY_ENV} is not usable: {e}")))
        })?;

    let source = build_source(&cfg)?;
    let slot = Arc::new(SnapshotSlot::empty());

    // Operational messages go to stderr, not stdout. Since M6 stdout carries
    // the access log and nothing else, so a log shipper can read it as a stream
    // of one shape instead of one shape with prose mixed in.
    eprintln!(
        "kallisto: serving {} on http://{} with {} worker(s), reading {}",
        cfg.mount,
        cfg.listen,
        cfg.workers,
        source.describe()
    );

    // stdout is the access log and nothing else from here on.
    let telemetry = Arc::new(Telemetry::with_log(
        cfg.log.queue_capacity,
        cfg.workers,
        cfg.log.enabled,
    ));
    let _writer = if cfg.log.enabled {
        Some(telemetry.log.spawn_writer(std::io::stdout()))
    } else {
        error_log!(
            "access log",
            "disabled by configuration; no reads will be recorded"
        );
        None
    };

    start_refresher(
        &cfg,
        source,
        key,
        Arc::clone(&slot),
        Arc::clone(&telemetry.refresh_failures),
    );

    let resolver = Resolver {
        slot,
        mount: cfg.mount.as_str().into(),
        limits: cfg.limits,
        telemetry,
    };
    let pool = WorkerPool::spawn(cfg.workers, cfg.listen, move |worker| {
        vault_api::router_for_worker(resolver.clone(), worker)
    });
    pool.join_all();
    Ok(())
}

fn build_source(cfg: &Config) -> Result<Arc<dyn SecretSource>, Startup> {
    match &cfg.source {
        Source::Disk { path } => Ok(Arc::new(DiskSource::new(path.clone()))),
        Source::Bucket {
            endpoint,
            bucket,
            object_key,
            region,
            path_style,
        } => {
            let endpoint = url::Url::parse(endpoint).map_err(|e| {
                Startup::Failed(format!("source.endpoint is not a URL: {endpoint:?}: {e}"))
            })?;
            let access_key_id = env(ACCESS_KEY_ENV).ok_or_else(|| {
                Startup::Failed(format!(
                    "{ACCESS_KEY_ENV} is not set, and the source is a bucket"
                ))
            })?;
            let secret_access_key = env(SECRET_KEY_ENV).ok_or_else(|| {
                Startup::Failed(format!(
                    "{SECRET_KEY_ENV} is not set, and the source is a bucket"
                ))
            })?;

            let source = BucketSource::new(BucketConfig {
                endpoint,
                bucket: bucket.clone(),
                region: region.clone(),
                object_key: object_key.clone(),
                path_style: *path_style,
                access_key_id,
                secret_access_key,
            })
            .map_err(|e| Startup::Failed(e.to_string()))?;
            Ok(Arc::new(source))
        }
    }
}

/// The poll loop gets a thread and a runtime of its own, and is deliberately
/// *not* pinned (ADR-0016 QĐ-3): a fifteen-second bucket timeout must never
/// occupy a core that is answering reads.
fn start_refresher(
    cfg: &Config,
    source: Arc<dyn SecretSource>,
    key: core_crypto::SealKey,
    slot: Arc<SnapshotSlot>,
    failures: Arc<std::sync::atomic::AtomicU64>,
) {
    let cache_path = cfg.cache_path.clone();
    let interval = cfg.refresh_interval;

    thread::Builder::new()
        .name("refresh".to_string())
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("refresh runtime");

            rt.block_on(async move {
                let mut refresher = Refresher::new(source, key, slot, cache_path).every(interval);

                // Cold start from the encrypted copy on this machine's disk
                // first, so the version it finds becomes the floor for the
                // anti-rollback check before the bucket is ever asked
                // (ADR-0015 D5).
                report("cache", refresher.warm_from_cache().await, &failures);
                report("source", refresher.poll_once().await, &failures);
                // Every later poll is reported too. Discarding these is how a
                // forged file in the bucket becomes invisible after the first
                // thirty seconds.
                refresher
                    .run(|tick| report("source", tick, &failures))
                    .await;
            });
        })
        .expect("failed to start the refresh thread");
}

/// What one poll of the source produced, on the error log.
///
/// `Tick` is safe to render: ADR-0015 D15 requires the error log to observe the
/// same hygiene as the access log, and the tamper tests hold every error
/// variant to carrying counts and positions rather than secret material, paths
/// or key bytes. That is what makes this call site safe, and it is checked
/// there rather than assumed here.
///
/// A rejection or an outage also increments the counter behind
/// `kallisto_refresh_failures_total`, so "this machine has been serving a stale
/// file for an hour" is something a scrape notices rather than something
/// somebody reads the logs to discover.
fn report(origin: &str, tick: Tick, failures: &std::sync::atomic::AtomicU64) {
    match tick {
        Tick::Loaded {
            version,
            cached,
            enforces,
        } => {
            eprintln!("kallisto: loaded version {version} from {origin}");
            if !enforces {
                eprintln!(
                    "kallisto: this file carries no token table, so every read is permitted and \
                     access log identifiers are keyed per process — they will not line up across \
                     a restart"
                );
            }
            if !cached {
                error_log!(
                    "fallback copy",
                    "could not be written — this machine will start sealed if the source is down"
                );
            }
        }
        Tick::Unchanged => {}
        Tick::Rejected(e) => {
            failures.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            error_log!("source", "refused the file offered by {origin}: {e}");
        }
        Tick::SourceDown(e) => {
            failures.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            error_log!("source", "{origin} unavailable: {e}");
        }
    }
}
