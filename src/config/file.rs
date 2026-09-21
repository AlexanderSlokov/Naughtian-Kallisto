//! The YAML file: its schema, and the checks that make it safe to commit.
//!
//! `deny_unknown_fields` everywhere, so a typo is an error instead of a
//! silently ignored line — and so a credential field has nowhere to parse into.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use super::ConfigError;

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
    pub address: Option<std::net::IpAddr>,
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

#[cfg(test)]
pub(super) mod tests {
    use super::*;

    pub(crate) const MINIMAL: &str = r#"
apiVersion: kallisto/v1
kind: Resolver
spec:
  source:
    type: disk
    path: /var/lib/kallisto/secrets.kal
"#;

    pub(crate) fn parse(text: &str) -> Result<FileConfig, ConfigError> {
        parse_file(text, Path::new("test.yaml"))
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

    /// The file is meant to be committable. If a credential field ever parses,
    /// someone will fill it in.
    #[test]
    fn credentials_have_no_place_to_go_in_the_file() {
        for line in [
            "    accessKeyId: AKIA",
            "    secretAccessKey: duck-fixture-not-a-credential",
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
