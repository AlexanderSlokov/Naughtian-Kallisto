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
    server::vault_api::{self, Resolver},
};

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
    let cli = config::parse_args(std::env::args().skip(1))?;
    let environment = config::from_env(env);

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

    println!(
        "kallisto: serving {} on http://{} with {} worker(s), reading {}",
        cfg.mount,
        cfg.listen,
        cfg.workers,
        source.describe()
    );

    start_refresher(&cfg, source, key, Arc::clone(&slot));

    let resolver = Resolver {
        slot,
        mount: cfg.mount.as_str().into(),
        limits: cfg.limits,
    };
    let pool = WorkerPool::spawn(cfg.workers, cfg.listen, move || {
        vault_api::router(resolver.clone())
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
                report("cache", refresher.warm_from_cache().await);
                report("source", refresher.poll_once().await);
                refresher.run().await;
            });
        })
        .expect("failed to start the refresh thread");
}

/// Until the access log lands (M6) this is the whole of the operator's view.
///
/// `Tick` is safe to print: ADR-0015 D15 and the tamper tests hold every error
/// variant to carrying no secret material, no path and no key bytes.
fn report(origin: &str, tick: Tick) {
    match tick {
        Tick::Loaded { version, cached } => {
            println!("kallisto: loaded version {version} from {origin}");
            if !cached {
                eprintln!(
                    "kallisto: could not write the encrypted fallback copy — this machine \
                     will start sealed if the source is down"
                );
            }
        }
        Tick::Unchanged => {}
        Tick::Rejected(e) => eprintln!("kallisto: refused the file offered by {origin}: {e}"),
        Tick::SourceDown(e) => eprintln!("kallisto: {origin} unavailable: {e}"),
    }
}
