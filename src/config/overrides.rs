//! The layers above the defaults: file, environment, command line. Each one is
//! read into an [`Overrides`], and later layers win field by field.

use std::{net::IpAddr, path::PathBuf, str::FromStr};

use super::{ConfigError, SEAL_KEY_ENV, Spec};

pub const USAGE: &str = "\
kallisto-server — local read-only secrets resolver

  --config=PATH            YAML configuration (env: KALLISTO_CONFIG)
  --listen-address=IP      address to bind          (env: KALLISTO_LISTEN_ADDRESS)
  --listen-port=PORT       port to bind             (env: KALLISTO_LISTEN_PORT)
  --workers=N              serving threads, one runtime per core (env: KALLISTO_WORKERS)
  --cache-dir=PATH         where the encrypted fallback copy is kept (env: KALLISTO_CACHE_DIR)
  --refresh-interval-seconds=N
                           how often to check the bucket (env: KALLISTO_REFRESH_INTERVAL_SECONDS)
  --i-accept-the-risk      permit binding a non-loopback address
  -h, --help               show this message

The seal key is read from KALLISTO_SEAL_KEY, and bucket credentials from
KALLISTO_S3_ACCESS_KEY_ID and KALLISTO_S3_SECRET_ACCESS_KEY. None of the three
can be given on the command line or written in the configuration file.
";

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
    /// The file's layer: the part of the spec that the environment and the
    /// command line can also set.
    pub(super) fn from_spec(spec: &Spec) -> Self {
        Self {
            config_path: None,
            address: spec.listen.as_ref().and_then(|l| l.address),
            port: spec.listen.as_ref().and_then(|l| l.port),
            workers: spec.workers,
            cache_dir: spec.cache_dir.clone(),
            refresh_interval_seconds: spec.refresh.as_ref().and_then(|r| r.interval_seconds),
            accept_risk: false,
        }
    }

    /// Later wins, which is why the caller passes `(file, env, cli)` in that
    /// order.
    pub(super) fn overlay(mut self, other: Overrides) -> Self {
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

/// Reads the layer between the file and the command line.
///
/// As strict as the file and the flags: a value that does not parse is a typo,
/// and a typo that quietly fell back to the default would run the process on a
/// port or with a worker count nobody asked for — a pod that is up and wrong,
/// instead of one that is down and says why. The error names the variable and
/// the value, so the fix is one line in the manifest.
///
/// An empty value counts as unset: it is what a templated manifest renders for
/// a value nobody filled in, and refusing to start over it helps nobody.
pub fn from_env<F: Fn(&str) -> Option<String>>(get: F) -> Result<Overrides, ConfigError> {
    let read = |var: &str| get(var).filter(|value| !value.is_empty());
    Ok(Overrides {
        config_path: read("KALLISTO_CONFIG").map(PathBuf::from),
        address: parse_env(&read, "KALLISTO_LISTEN_ADDRESS", "an IP")?,
        port: parse_env(&read, "KALLISTO_LISTEN_PORT", "a port")?,
        workers: parse_env(&read, "KALLISTO_WORKERS", "a number")?,
        cache_dir: read("KALLISTO_CACHE_DIR").map(PathBuf::from),
        refresh_interval_seconds: parse_env(
            &read,
            "KALLISTO_REFRESH_INTERVAL_SECONDS",
            "a number",
        )?,
        accept_risk: false,
    })
}

fn parse_env<T: FromStr>(
    read: &impl Fn(&str) -> Option<String>,
    var: &str,
    expected: &str,
) -> Result<Option<T>, ConfigError> {
    read(var)
        .map(|raw| parse_value(var, raw, expected))
        .transpose()
}

/// `--flag=value` and `--flag value`, same as the binary has always accepted.
pub fn parse_args<I: Iterator<Item = String>>(mut args: I) -> Result<Overrides, ConfigError> {
    let mut out = Overrides::default();
    while let Some(arg) = args.next() {
        let (flag, inline) = match arg.split_once('=') {
            Some((f, v)) => (f.to_string(), Some(v.to_string())),
            None => (arg, None),
        };
        let value = || match inline {
            Some(v) => Ok(v),
            None => args
                .next()
                .ok_or_else(|| ConfigError::Usage(format!("{flag} requires a value"))),
        };
        apply_flag(&mut out, &flag, value)?;
    }
    Ok(out)
}

/// One flag, applied. `value` is only called by the flags that take one, so a
/// bare `--i-accept-the-risk` never swallows the argument after it.
fn apply_flag(
    out: &mut Overrides,
    flag: &str,
    value: impl FnOnce() -> Result<String, ConfigError>,
) -> Result<(), ConfigError> {
    match flag {
        "--config" => out.config_path = Some(PathBuf::from(value()?)),
        "--listen-address" => out.address = Some(parse_value(flag, value()?, "an IP")?),
        "--listen-port" => out.port = Some(parse_value(flag, value()?, "a port")?),
        "--workers" => out.workers = Some(parse_value(flag, value()?, "a number")?),
        "--cache-dir" => out.cache_dir = Some(PathBuf::from(value()?)),
        "--refresh-interval-seconds" => {
            out.refresh_interval_seconds = Some(parse_value(flag, value()?, "a number")?);
        }
        "--i-accept-the-risk" => out.accept_risk = true,
        refused => return Err(refusal(refused)),
    }
    Ok(())
}

/// Every flag that is not applied, and why: help, the seal key, and anything
/// unrecognised.
fn refusal(flag: &str) -> ConfigError {
    match flag {
        "-h" | "--help" => ConfigError::Usage(USAGE.to_string()),
        // A rejected seal key is worth a specific message: someone doing this
        // is one step away from putting the key in a shell history file, a CI
        // log and a process listing at once.
        "--seal-key" => ConfigError::Usage(format!(
            "the seal key is never a command-line argument — every process on \
             this host can read those. Set {SEAL_KEY_ENV} instead."
        )),
        other => ConfigError::Usage(format!("unrecognised argument {other:?}\n\n{USAGE}")),
    }
}

fn parse_value<T: FromStr>(flag: &str, raw: String, expected: &str) -> Result<T, ConfigError> {
    raw.parse()
        .map_err(|_| ConfigError::Usage(format!("{flag} expects {expected}, got {raw:?}")))
}

#[cfg(test)]
mod tests {
    use super::*;

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

    fn env_of<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        |var| {
            pairs
                .iter()
                .find(|(name, _)| *name == var)
                .map(|(_, value)| (*value).to_string())
        }
    }

    /// A typo in the environment stops the process, naming the variable and
    /// what it held — the same as a typo in a flag or in the file.
    #[test]
    fn a_malformed_environment_value_is_an_error_not_a_silent_default() {
        for (var, raw) in [
            ("KALLISTO_LISTEN_ADDRESS", "0.0.0.O"),
            ("KALLISTO_LISTEN_PORT", "82OO"),
            ("KALLISTO_WORKERS", "abc"),
            ("KALLISTO_REFRESH_INTERVAL_SECONDS", "30s"),
        ] {
            let message = from_env(env_of(&[(var, raw)])).unwrap_err().to_string();
            assert!(
                message.contains(var),
                "{var}: message does not name it: {message}"
            );
            assert!(
                message.contains(raw),
                "{var}: message does not quote {raw:?}: {message}"
            );
        }
    }

    /// What a templated manifest renders for a value nobody filled in.
    #[test]
    fn an_empty_environment_value_counts_as_unset() {
        let overrides = from_env(env_of(&[
            ("KALLISTO_CONFIG", ""),
            ("KALLISTO_LISTEN_PORT", ""),
            ("KALLISTO_WORKERS", ""),
        ]))
        .unwrap();
        assert_eq!(overrides.config_path, None);
        assert_eq!(overrides.port, None);
        assert_eq!(overrides.workers, None);
    }

    /// `--refresh-interval-seconds` was accepted for a while without appearing
    /// in `--help`. Every flag that is applied has to be one a user can find.
    #[test]
    fn every_accepted_flag_is_listed_in_the_usage_text() {
        for (flag, sample) in [
            ("--config", "/x.yaml"),
            ("--listen-address", "127.0.0.1"),
            ("--listen-port", "8200"),
            ("--workers", "2"),
            ("--cache-dir", "/tmp"),
            ("--refresh-interval-seconds", "30"),
            ("--i-accept-the-risk", ""),
        ] {
            let arg = if sample.is_empty() {
                flag.to_string()
            } else {
                format!("{flag}={sample}")
            };
            parse_args([arg].into_iter()).unwrap();
            assert!(USAGE.contains(flag), "{flag} is accepted but not in USAGE");
        }
    }
}
