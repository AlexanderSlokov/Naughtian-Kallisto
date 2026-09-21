//! Configuration, at last (ADR-0003).
//!
//! ADR-0003 was accepted in August and never implemented: the binary read a
//! handful of command-line flags and hardcoded everything else. This is the
//! file it asked for — Kubernetes-shaped YAML, `deny_unknown_fields` so a typo
//! is an error instead of a silently ignored line, and the precedence it
//! specified: **CLI > env > file > defaults**.
//!
//! Two rules from ADR-0003 and ADR-0015 are enforced here rather than
//! documented:
//!
//! * **No secret ever appears in the file.** Bucket credentials and the seal
//!   key are read from the environment only, so `kallisto.yaml` can live in git
//!   next to the Deployment that mounts it.
//! * **The listener is loopback unless someone says otherwise in writing.**
//!   ADR-0015's first operating constraint is that port 8200 is never exposed
//!   beyond localhost. A constraint that lives only in a document is a
//!   constraint that gets violated on a Friday, so a non-loopback address
//!   refuses to start without `--i-accept-the-risk`.
//!
//! Laid out the way the precedence reads: [`file`] is the schema and its
//! checks, [`overrides`] is every layer that can override it, and this module
//! is the defaults and the one function, [`Config::resolve`], that merges them.

mod file;
mod overrides;

use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::PathBuf,
    time::Duration,
};

pub use file::{
    API_VERSION, FileConfig, KIND, Limits, Listen, Log, Refresh, SourceSpec, Spec, parse_file,
    read_file,
};
pub use overrides::{Overrides, USAGE, from_env, parse_args};

pub const DEFAULT_PORT: u16 = 8200;
pub const DEFAULT_WORKERS: usize = 2;
pub const DEFAULT_MOUNT: &str = "secret";
/// Per worker, not per process — see [`Limits`].
pub const DEFAULT_RPS: u64 = 20_000;
/// Access log lines buffered before the process starts dropping them.
///
/// 8192 lines is roughly a tenth of a second at this machine's measured
/// ceiling, which is the right order: long enough to ride out a disk hiccup,
/// short enough that a sustained flood shows up as dropped lines — the signal
/// QĐ-8 wants raised — rather than as a slowly growing buffer.
pub const DEFAULT_LOG_QUEUE: usize = 8192;

const DEFAULT_ADDRESS: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);
/// The name of the encrypted fallback copy inside `--cache-dir`.
const CACHE_FILE_NAME: &str = "secrets.kal";

/// The environment variable holding the 32-byte seal key, hex-encoded.
///
/// Deliberately not a flag. Command-line arguments are readable by every
/// process on the host through `ps`; environment variables of another process
/// are not, on a normally configured Linux.
pub const SEAL_KEY_ENV: &str = "KALLISTO_SEAL_KEY";
pub const ACCESS_KEY_ENV: &str = "KALLISTO_S3_ACCESS_KEY_ID";
pub const SECRET_KEY_ENV: &str = "KALLISTO_S3_SECRET_ACCESS_KEY";

/// The resolved thing the program actually runs on.
#[derive(Debug, Clone)]
pub struct Config {
    pub listen: SocketAddr,
    pub workers: usize,
    pub mount: String,
    pub source: Source,
    pub refresh_interval: Duration,
    pub cache_path: Option<PathBuf>,
    pub limits: ResolvedLimits,
    pub log: ResolvedLog,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolvedLog {
    pub queue_capacity: usize,
    pub enabled: bool,
}

#[derive(Debug, Clone)]
pub enum Source {
    Bucket {
        endpoint: String,
        bucket: String,
        object_key: String,
        region: String,
        path_style: bool,
    },
    Disk {
        path: PathBuf,
    },
}

#[derive(Debug, Clone, Copy)]
pub struct ResolvedLimits {
    pub requests_per_second: u64,
    pub burst: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("{0}")]
    Usage(String),
    #[error("could not read {path}: {source}")]
    Unreadable {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("{path} is not valid configuration: {message}")]
    Malformed { path: PathBuf, message: String },
    #[error("apiVersion must be {expected:?}, got {got:?}")]
    ApiVersion { expected: &'static str, got: String },
    #[error("kind must be {expected:?}, got {got:?}")]
    Kind { expected: &'static str, got: String },
    #[error(
        "refusing to bind {address}: Kallisto answers on localhost only (ADR-0015). \
         Pass --i-accept-the-risk if you have a network you trust more than we do."
    )]
    NotLoopback { address: IpAddr },
    #[error("workers must be at least 1")]
    NoWorkers,
    #[error("no configuration: pass --config, or set KALLISTO_CONFIG")]
    Missing,
}

impl Config {
    /// The one entry point: defaults, then file, then env, then CLI.
    pub fn resolve(file: FileConfig, env: Overrides, cli: Overrides) -> Result<Self, ConfigError> {
        let spec = file.spec;
        let merged = Overrides::from_spec(&spec).overlay(env).overlay(cli);

        Ok(Config {
            listen: SocketAddr::new(bind_address(&merged)?, merged.port.unwrap_or(DEFAULT_PORT)),
            workers: worker_count(&merged)?,
            mount: spec.mount.unwrap_or_else(|| DEFAULT_MOUNT.to_string()),
            source: spec.source.into(),
            refresh_interval: refresh_interval(&merged),
            cache_path: merged.cache_dir.map(|dir| dir.join(CACHE_FILE_NAME)),
            limits: ResolvedLimits::from_spec(spec.limits.as_ref()),
            log: ResolvedLog::from_spec(spec.log.as_ref()),
        })
    }
}

/// The first operating constraint of ADR-0015, enforced rather than documented.
fn bind_address(merged: &Overrides) -> Result<IpAddr, ConfigError> {
    let address = merged.address.unwrap_or(DEFAULT_ADDRESS);
    if !address.is_loopback() && !merged.accept_risk {
        return Err(ConfigError::NotLoopback { address });
    }
    Ok(address)
}

fn worker_count(merged: &Overrides) -> Result<usize, ConfigError> {
    let workers = merged.workers.unwrap_or(DEFAULT_WORKERS);
    if workers == 0 {
        return Err(ConfigError::NoWorkers);
    }
    Ok(workers)
}

fn refresh_interval(merged: &Overrides) -> Duration {
    let default = crate::resolver::refresh::DEFAULT_INTERVAL.as_secs();
    Duration::from_secs(merged.refresh_interval_seconds.unwrap_or(default))
}

impl ResolvedLimits {
    fn from_spec(limits: Option<&Limits>) -> Self {
        let rps = limits
            .and_then(|l| l.requests_per_second_per_worker)
            .unwrap_or(DEFAULT_RPS);
        Self {
            requests_per_second: rps,
            // A burst of one second's worth: enough to absorb a thundering
            // herd of sidecars restarting together, not enough to hide a
            // runaway loop.
            burst: limits.and_then(|l| l.burst).unwrap_or(rps),
        }
    }
}

impl ResolvedLog {
    fn from_spec(log: Option<&Log>) -> Self {
        Self {
            queue_capacity: log
                .and_then(|l| l.queue_capacity)
                .unwrap_or(DEFAULT_LOG_QUEUE)
                .max(2),
            enabled: log.and_then(|l| l.enabled).unwrap_or(true),
        }
    }
}

impl From<SourceSpec> for Source {
    fn from(spec: SourceSpec) -> Self {
        match spec {
            SourceSpec::Bucket {
                endpoint,
                bucket,
                object_key,
                region,
                path_style,
            } => Source::Bucket {
                endpoint,
                bucket,
                object_key,
                region,
                path_style,
            },
            SourceSpec::Disk { path } => Source::Disk { path },
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{
        file::tests::{MINIMAL, parse},
        *,
    };

    fn resolve(text: &str, cli: &[&str]) -> Result<Config, ConfigError> {
        let file = parse(text)?;
        let cli = parse_args(cli.iter().map(|s| (*s).to_string()))?;
        Config::resolve(file, Overrides::default(), cli)
    }

    /// ADR-0003 promised a working configuration under ten lines.
    #[test]
    fn the_minimal_configuration_is_short_and_works() {
        assert!(MINIMAL.trim().lines().count() <= 10);
        let cfg = resolve(MINIMAL, &[]).unwrap();
        assert_eq!(cfg.listen.port(), DEFAULT_PORT);
        assert_eq!(cfg.workers, DEFAULT_WORKERS);
        assert_eq!(cfg.mount, "secret");
        assert_eq!(cfg.refresh_interval, Duration::from_secs(30));
    }

    /// The first operating constraint of ADR-0015, enforced rather than
    /// documented.
    #[test]
    fn the_default_bind_is_loopback() {
        assert!(resolve(MINIMAL, &[]).unwrap().listen.ip().is_loopback());
    }

    #[test]
    fn a_public_address_is_refused_unless_the_risk_is_accepted() {
        let err = resolve(MINIMAL, &["--listen-address=0.0.0.0"]).unwrap_err();
        assert!(
            matches!(err, ConfigError::NotLoopback { .. }),
            "expected a refusal, got {err}"
        );

        let ok = resolve(
            MINIMAL,
            &["--listen-address=0.0.0.0", "--i-accept-the-risk"],
        )
        .unwrap();
        assert_eq!(ok.listen.ip().to_string(), "0.0.0.0");
    }

    /// ::1 is loopback too, and someone will run this in an IPv6-only pod.
    #[test]
    fn ipv6_loopback_counts_as_loopback() {
        resolve(MINIMAL, &["--listen-address=::1"]).unwrap();
    }

    #[test]
    fn precedence_is_cli_over_env_over_file_over_defaults() {
        let with_port = r#"
apiVersion: kallisto/v1
kind: Resolver
spec:
  listen:
    port: 1111
  workers: 4
  source:
    type: disk
    path: /x.kal
"#;
        // file alone
        let file_only = Config::resolve(
            parse(with_port).unwrap(),
            Overrides::default(),
            Overrides::default(),
        )
        .unwrap();
        assert_eq!(file_only.listen.port(), 1111);
        assert_eq!(file_only.workers, 4);

        // env beats file
        let env = from_env(|k| match k {
            "KALLISTO_LISTEN_PORT" => Some("2222".to_string()),
            "KALLISTO_WORKERS" => Some("6".to_string()),
            _ => None,
        });
        let env_wins =
            Config::resolve(parse(with_port).unwrap(), env, Overrides::default()).unwrap();
        assert_eq!(env_wins.listen.port(), 2222);
        assert_eq!(env_wins.workers, 6);

        // cli beats env
        let env = from_env(|k| match k {
            "KALLISTO_LISTEN_PORT" => Some("2222".to_string()),
            _ => None,
        });
        let cli = parse_args(["--listen-port=3333".to_string()].into_iter()).unwrap();
        let cli_wins = Config::resolve(parse(with_port).unwrap(), env, cli).unwrap();
        assert_eq!(cli_wins.listen.port(), 3333);
    }

    #[test]
    fn bad_arguments_are_rejected_rather_than_ignored() {
        parse_args(["--workers=zero".to_string()].into_iter()).unwrap_err();
        parse_args(["--nonsense".to_string()].into_iter()).unwrap_err();
        parse_args(["--config".to_string()].into_iter()).unwrap_err();
        resolve(MINIMAL, &["--workers=0"]).unwrap_err();
    }

    #[test]
    fn a_bucket_source_carries_its_addressing_style() {
        let yaml = r#"
apiVersion: kallisto/v1
kind: Resolver
spec:
  source:
    type: bucket
    endpoint: https://s3.example.com
    bucket: secrets
    objectKey: prod/payment.kal
    pathStyle: true
"#;
        let cfg = resolve(yaml, &[]).unwrap();
        match cfg.source {
            Source::Bucket {
                path_style, region, ..
            } => {
                assert!(path_style);
                assert_eq!(region, "auto");
            }
            other => panic!("expected a bucket, got {other:?}"),
        }
    }

    /// An example configuration that does not parse is worse than none: the
    /// first thing anyone does is copy it.
    #[test]
    fn the_shipped_example_configuration_parses() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("kallisto.example.yaml");
        let text = std::fs::read_to_string(&path).expect("kallisto.example.yaml is missing");
        let cfg = Config::resolve(
            parse_file(&text, &path).unwrap(),
            Overrides::default(),
            Overrides::default(),
        )
        .unwrap();
        assert!(cfg.listen.ip().is_loopback());
        assert!(matches!(cfg.source, Source::Bucket { .. }));
        assert_eq!(cfg.limits.requests_per_second, 20_000);
    }
}
