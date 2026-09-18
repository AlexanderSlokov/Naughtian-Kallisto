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

use std::{
    net::{IpAddr, SocketAddr},
    path::{Path, PathBuf},
    time::Duration,
};

use serde::Deserialize;

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

/// The environment variable holding the 32-byte seal key, hex-encoded.
///
/// Deliberately not a flag. Command-line arguments are readable by every
/// process on the host through `ps`; environment variables of another process
/// are not, on a normally configured Linux.
pub const SEAL_KEY_ENV: &str = "KALLISTO_SEAL_KEY";
pub const ACCESS_KEY_ENV: &str = "KALLISTO_S3_ACCESS_KEY_ID";
pub const SECRET_KEY_ENV: &str = "KALLISTO_S3_SECRET_ACCESS_KEY";

pub const USAGE: &str = "\
kallisto-server — local read-only secrets resolver

  --config=PATH            YAML configuration (env: KALLISTO_CONFIG)
  --listen-address=IP      address to bind          (env: KALLISTO_LISTEN_ADDRESS)
  --listen-port=PORT       port to bind             (env: KALLISTO_LISTEN_PORT)
  --workers=N              serving threads, one runtime per core
  --cache-dir=PATH         where the encrypted fallback copy is kept
  --i-accept-the-risk      permit binding a non-loopback address
  -h, --help               show this message

The seal key is read from KALLISTO_SEAL_KEY, and bucket credentials from
KALLISTO_S3_ACCESS_KEY_ID and KALLISTO_S3_SECRET_ACCESS_KEY. None of the three
can be given on the command line or written in the configuration file.
";

// -----------------------------------------------------------------------------
// The file
// -----------------------------------------------------------------------------

/// `apiVersion: kallisto/v1`, so a future schema change is a new value here
/// rather than a breaking edit to this one (ADR-0003).
pub const API_VERSION: &str = "kallisto/v1";
pub const KIND: &str = "Resolver";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileConfig {
    #[serde(rename = "apiVersion")]
    pub api_version: String,
    pub kind: String,
    pub spec: Spec,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Spec {
    #[serde(default)]
    pub listen: Option<Listen>,
    #[serde(default)]
    pub workers: Option<usize>,
    /// The KV-v2 mount Kallisto answers on. `secret` unless someone had a
    /// reason.
    #[serde(default)]
    pub mount: Option<String>,
    pub source: SourceSpec,
    #[serde(default)]
    pub refresh: Option<Refresh>,
    #[serde(default)]
    pub cache_dir: Option<PathBuf>,
    #[serde(default)]
    pub limits: Option<Limits>,
    #[serde(default)]
    pub log: Option<Log>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Listen {
    #[serde(default)]
    pub address: Option<IpAddr>,
    #[serde(default)]
    pub port: Option<u16>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Refresh {
    pub interval_seconds: Option<u64>,
}

/// Rates are **per worker**. Each worker owns its own counter so the serving
/// path never touches a shared cache line (ADR-0016 QĐ-3); a process-wide
/// figure would have to be one, and a contended atomic on the read path is
/// exactly what that decision exists to avoid.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Limits {
    #[serde(default)]
    pub requests_per_second_per_worker: Option<u64>,
    #[serde(default)]
    pub burst: Option<u64>,
}

/// The access log (ADR-0015 D15).
///
/// There is no setting here to make it an audit log, and there will not be
/// one: an audit log has to record before it serves, which means a full queue
/// stops the machine. That is a different product decision, not a flag. See the
/// module docs of the `telemetry` crate.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Log {
    /// Lines buffered between the workers and the writer. When it fills, lines
    /// are dropped and counted — never queued elsewhere, never waited on.
    #[serde(default)]
    pub queue_capacity: Option<usize>,
    #[serde(default)]
    pub enabled: Option<bool>,
}

/// Tagged so that a bucket field under a disk source fails to parse, which is
/// the property ADR-0003 wanted tagged enums for.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, tag = "type", rename_all = "camelCase")]
pub enum SourceSpec {
    /// An S3-compatible bucket: R2, Garage, MinIO, AWS.
    #[serde(rename_all = "camelCase")]
    Bucket {
        /// Scheme and authority only, e.g. `https://s3.example.com`.
        endpoint: String,
        bucket: String,
        object_key: String,
        #[serde(default = "default_region")]
        region: String,
        /// Garage and MinIO want path style; R2 and AWS take virtual-host.
        #[serde(default)]
        path_style: bool,
    },
    /// A local file. Used by the test suite, and by anyone who ships the
    /// sealed file with the image instead of fetching it.
    #[serde(rename_all = "camelCase")]
    Disk { path: PathBuf },
}

fn default_region() -> String {
    "auto".to_string()
}

// -----------------------------------------------------------------------------
// The resolved thing the program actually runs on
// -----------------------------------------------------------------------------

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

/// Values that can arrive from more than one layer. Each layer produces one of
/// these; later layers overwrite earlier ones field by field, which is the
/// whole of the CLI > env > file > defaults rule.
#[derive(Debug, Default)]
pub struct Overrides {
    pub config_path: Option<PathBuf>,
    pub address: Option<IpAddr>,
    pub port: Option<u16>,
    pub workers: Option<usize>,
    pub cache_dir: Option<PathBuf>,
    pub refresh_interval_seconds: Option<u64>,
    pub accept_risk: bool,
}

impl Overrides {
    /// Later wins, which is why the caller passes `(file, env, cli)` in that
    /// order.
    fn overlay(mut self, other: Overrides) -> Self {
        self.config_path = other.config_path.or(self.config_path);
        self.address = other.address.or(self.address);
        self.port = other.port.or(self.port);
        self.workers = other.workers.or(self.workers);
        self.cache_dir = other.cache_dir.or(self.cache_dir);
        self.refresh_interval_seconds = other
            .refresh_interval_seconds
            .or(self.refresh_interval_seconds);
        self.accept_risk |= other.accept_risk;
        self
    }
}

/// `--flag=value` and `--flag value`, same as the binary has always accepted.
pub fn parse_args<I: Iterator<Item = String>>(args: I) -> Result<Overrides, ConfigError> {
    let mut out = Overrides::default();
    let mut args = args.peekable();

    while let Some(arg) = args.next() {
        let (flag, inline) = match arg.split_once('=') {
            Some((f, v)) => (f.to_string(), Some(v.to_string())),
            None => (arg, None),
        };

        let mut value = |flag: &str| -> Result<String, ConfigError> {
            match inline.clone() {
                Some(v) => Ok(v),
                None => args
                    .next()
                    .ok_or_else(|| ConfigError::Usage(format!("{flag} requires a value"))),
            }
        };

        match flag.as_str() {
            "--config" => out.config_path = Some(PathBuf::from(value("--config")?)),
            "--listen-address" => {
                let raw = value("--listen-address")?;
                out.address = Some(raw.parse().map_err(|_| {
                    ConfigError::Usage(format!("--listen-address expects an IP, got {raw:?}"))
                })?);
            }
            "--listen-port" => {
                let raw = value("--listen-port")?;
                out.port = Some(raw.parse().map_err(|_| {
                    ConfigError::Usage(format!("--listen-port expects a port, got {raw:?}"))
                })?);
            }
            "--workers" => {
                let raw = value("--workers")?;
                out.workers = Some(raw.parse().map_err(|_| {
                    ConfigError::Usage(format!("--workers expects a number, got {raw:?}"))
                })?);
            }
            "--cache-dir" => out.cache_dir = Some(PathBuf::from(value("--cache-dir")?)),
            "--refresh-interval-seconds" => {
                let raw = value("--refresh-interval-seconds")?;
                out.refresh_interval_seconds = Some(raw.parse().map_err(|_| {
                    ConfigError::Usage(format!(
                        "--refresh-interval-seconds expects a number, got {raw:?}"
                    ))
                })?);
            }
            "--i-accept-the-risk" => out.accept_risk = true,
            "-h" | "--help" => return Err(ConfigError::Usage(USAGE.to_string())),
            // A rejected seal key is worth a specific message: someone doing
            // this is one step away from putting the key in a shell history
            // file, a CI log and a process listing at once.
            "--seal-key" => {
                return Err(ConfigError::Usage(format!(
                    "the seal key is never a command-line argument — every process on \
                     this host can read those. Set {SEAL_KEY_ENV} instead."
                )));
            }
            other => {
                return Err(ConfigError::Usage(format!(
                    "unrecognised argument {other:?}\n\n{USAGE}"
                )));
            }
        }
    }
    Ok(out)
}

/// Reads the layer between the file and the command line.
pub fn from_env<F: Fn(&str) -> Option<String>>(get: F) -> Overrides {
    Overrides {
        config_path: get("KALLISTO_CONFIG").map(PathBuf::from),
        address: get("KALLISTO_LISTEN_ADDRESS").and_then(|v| v.parse().ok()),
        port: get("KALLISTO_LISTEN_PORT").and_then(|v| v.parse().ok()),
        workers: get("KALLISTO_WORKERS").and_then(|v| v.parse().ok()),
        cache_dir: get("KALLISTO_CACHE_DIR").map(PathBuf::from),
        refresh_interval_seconds: get("KALLISTO_REFRESH_INTERVAL_SECONDS")
            .and_then(|v| v.parse().ok()),
        accept_risk: false,
    }
}

pub fn read_file(path: &Path) -> Result<FileConfig, ConfigError> {
    let text = std::fs::read_to_string(path).map_err(|source| ConfigError::Unreadable {
        path: path.to_path_buf(),
        source,
    })?;
    parse_file(&text, path)
}

pub fn parse_file(text: &str, path: &Path) -> Result<FileConfig, ConfigError> {
    let file: FileConfig = serde_norway::from_str(text).map_err(|e| ConfigError::Malformed {
        path: path.to_path_buf(),
        message: e.to_string(),
    })?;

    if file.api_version != API_VERSION {
        return Err(ConfigError::ApiVersion {
            expected: API_VERSION,
            got: file.api_version,
        });
    }
    if file.kind != KIND {
        return Err(ConfigError::Kind {
            expected: KIND,
            got: file.kind,
        });
    }
    Ok(file)
}

impl Config {
    /// The one entry point: defaults, then file, then env, then CLI.
    pub fn resolve(file: FileConfig, env: Overrides, cli: Overrides) -> Result<Self, ConfigError> {
        let spec = file.spec;

        let from_file = Overrides {
            config_path: None,
            address: spec.listen.as_ref().and_then(|l| l.address),
            port: spec.listen.as_ref().and_then(|l| l.port),
            workers: spec.workers,
            cache_dir: spec.cache_dir.clone(),
            refresh_interval_seconds: spec.refresh.as_ref().and_then(|r| r.interval_seconds),
            accept_risk: false,
        };
        let merged = from_file.overlay(env).overlay(cli);

        let address = merged.address.unwrap_or(IpAddr::from([127, 0, 0, 1]));
        if !address.is_loopback() && !merged.accept_risk {
            return Err(ConfigError::NotLoopback { address });
        }

        let workers = merged.workers.unwrap_or(DEFAULT_WORKERS);
        if workers == 0 {
            return Err(ConfigError::NoWorkers);
        }

        let limits = spec.limits.as_ref();
        let rps = limits
            .and_then(|l| l.requests_per_second_per_worker)
            .unwrap_or(DEFAULT_RPS);

        Ok(Config {
            listen: SocketAddr::new(address, merged.port.unwrap_or(DEFAULT_PORT)),
            workers,
            mount: spec.mount.unwrap_or_else(|| DEFAULT_MOUNT.to_string()),
            source: match spec.source {
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
            },
            refresh_interval: Duration::from_secs(
                merged
                    .refresh_interval_seconds
                    .unwrap_or(crate::resolver::refresh::DEFAULT_INTERVAL.as_secs()),
            ),
            cache_path: merged.cache_dir.map(|dir| dir.join("secrets.kal")),
            limits: ResolvedLimits {
                requests_per_second: rps,
                // A burst of one second's worth: enough to absorb a thundering
                // herd of sidecars restarting together, not enough to hide a
                // runaway loop.
                burst: limits.and_then(|l| l.burst).unwrap_or(rps),
            },
            log: ResolvedLog {
                queue_capacity: spec
                    .log
                    .as_ref()
                    .and_then(|l| l.queue_capacity)
                    .unwrap_or(DEFAULT_LOG_QUEUE)
                    .max(2),
                enabled: spec.log.as_ref().and_then(|l| l.enabled).unwrap_or(true),
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINIMAL: &str = r#"
apiVersion: kallisto/v1
kind: Resolver
spec:
  source:
    type: disk
    path: /var/lib/kallisto/secrets.kal
"#;

    fn parse(text: &str) -> Result<FileConfig, ConfigError> {
        parse_file(text, Path::new("test.yaml"))
    }

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

    /// The point of `deny_unknown_fields`: a typo is not a shrug.
    #[test]
    fn a_misspelled_field_is_an_error_not_a_silent_default() {
        let typo = r#"
apiVersion: kallisto/v1
kind: Resolver
spec:
  wokers: 4
  source:
    type: disk
    path: /x.kal
"#;
        let err = parse(typo).unwrap_err();
        assert!(matches!(err, ConfigError::Malformed { .. }), "got {err}");
    }

    /// ADR-0003 wanted the tagged enum so an impossible combination cannot
    /// parse, rather than being ignored at runtime.
    #[test]
    fn bucket_fields_under_a_disk_source_do_not_parse() {
        let mixed = r#"
apiVersion: kallisto/v1
kind: Resolver
spec:
  source:
    type: disk
    path: /x.kal
    bucket: nope
"#;
        parse(mixed).unwrap_err();
    }

    #[test]
    fn an_unknown_api_version_is_named_as_such() {
        let future = MINIMAL.replace("kallisto/v1", "kallisto/v2");
        assert!(matches!(
            parse(&future).unwrap_err(),
            ConfigError::ApiVersion { .. }
        ));
    }

    #[test]
    fn the_seal_key_cannot_be_passed_as_an_argument() {
        let err = parse_args(["--seal-key=deadbeef".to_string()].into_iter()).unwrap_err();
        let message = err.to_string();
        assert!(
            message.contains(SEAL_KEY_ENV),
            "unhelpful message: {message}"
        );
        assert!(
            !message.contains("deadbeef"),
            "the refusal echoed the key back: {message}"
        );
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

    /// The file is meant to be committable. If a credential field ever parses,
    /// someone will fill it in.
    #[test]
    fn credentials_have_no_place_to_go_in_the_file() {
        for line in [
            "    accessKeyId: AKIA",
            "    secretAccessKey: hunter2",
            "    sealKey: 00",
        ] {
            let yaml = format!(
                "apiVersion: kallisto/v1\nkind: Resolver\nspec:\n  source:\n    type: bucket\n    endpoint: https://s3.example.com\n    bucket: b\n    objectKey: k\n{line}\n"
            );
            assert!(
                parse(&yaml).is_err(),
                "the configuration file accepted a credential: {line}"
            );
        }
    }
}
