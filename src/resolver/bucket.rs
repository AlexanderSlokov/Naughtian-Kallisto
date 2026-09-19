//! Reading the sealed file from an S3-compatible bucket (ADR-0015 D2).
//!
//! Only the read half of S3 is implemented, which is what makes "R2, Garage,
//! MinIO or AWS" a true statement rather than a hope: a presigned GET with
//! `If-None-Match` is the one thing all of them implement identically.
//!
//! Signing is [`super::sigv4`], written in this repo. The official AWS SDK is
//! ruled out by ADR-0015 D2 on binary size, and every S3 signing crate on
//! crates.io reaches for RustCrypto's `hmac`/`sha2`, which `deny.toml` bans.

use std::{fmt, time::Duration};

use reqwest::{Client, StatusCode, header};

use super::{
    sigv4::{SigningInputs, presign_query, uri_encode},
    source::{BoxFuture, Fetched, SecretSource, SourceError},
};

/// How long a signed URL stays valid. Short, because it is used immediately;
/// long enough to survive a slow link and a little clock skew.
const SIGNATURE_TTL_SECS: u32 = 60;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

pub struct BucketConfig {
    /// Scheme and authority only, e.g. `https://s3.example.com`.
    pub endpoint: url::Url,
    pub bucket: String,
    pub region: String,
    /// Object key of the sealed file, e.g. `prod/payment.kal`.
    pub object_key: String,
    /// Garage and MinIO want path style; R2 and AWS take virtual-host style.
    pub path_style: bool,
    pub access_key_id: String,
    pub secret_access_key: String,
}

/// Names the location, never the credentials.
impl fmt::Debug for BucketConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BucketConfig")
            .field("endpoint", &self.endpoint.as_str())
            .field("bucket", &self.bucket)
            .field("object_key", &self.object_key)
            .field("access_key_id", &"<REDACTED>")
            .field("secret_access_key", &"<REDACTED>")
            .finish()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum BucketError {
    #[error("bucket endpoint {endpoint:?} has no host")]
    NoHost { endpoint: String },
    #[error("could not build an HTTP client: {0}")]
    Client(String),
}

pub struct BucketSource {
    config: BucketConfig,
    /// `scheme://host` with no trailing slash, and the host as it must appear
    /// in the signed `host` header. Both are fixed at construction because a
    /// mismatch between them is a 403 that is miserable to diagnose.
    origin: String,
    host: String,
    canonical_uri: String,
    client: Client,
    description: String,
}

impl BucketSource {
    pub fn new(config: BucketConfig) -> Result<Self, BucketError> {
        let host = signed_host(&config)?;
        let origin = format!("{}://{host}", config.endpoint.scheme());
        let canonical_uri = canonical_uri(&config);

        let client = Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(|e| BucketError::Client(e.to_string()))?;

        let description = format!("s3://{}/{}", config.bucket, config.object_key);
        Ok(Self {
            config,
            origin,
            host,
            canonical_uri,
            client,
            description,
        })
    }

    fn signed_url(&self, now_secs: u64) -> String {
        let query = presign_query(&SigningInputs {
            access_key_id: &self.config.access_key_id,
            secret_access_key: &self.config.secret_access_key,
            region: &self.config.region,
            host: &self.host,
            canonical_uri: &self.canonical_uri,
            expires_secs: SIGNATURE_TTL_SECS,
            now_secs,
        });
        format!("{}{}?{query}", self.origin, self.canonical_uri)
    }

    async fn get(&self, etag: Option<&str>) -> Fetched {
        let mut request = self.client.get(self.signed_url(now_secs()));

        // The conditional GET ADR-0015 D2 makes mandatory. On the overwhelming
        // majority of polls the file has not changed, and this is what keeps
        // ten VMs polling every thirty seconds inside R2's free tier.
        if let Some(etag) = etag {
            request = request.header(header::IF_NONE_MATCH, etag);
        }

        let response = match request.send().await {
            Ok(response) => response,
            Err(e) => return Fetched::Unavailable(transport_error(&e)),
        };

        match response.status() {
            StatusCode::NOT_MODIFIED => Fetched::NotModified,
            StatusCode::OK => body_of(response).await,
            StatusCode::NOT_FOUND => Fetched::Unavailable(SourceError::Absent),
            // ADR-0015 D14: this dialect stops here. The app never sees it — it
            // gets Vault's 429, and only when *we* are the overloaded one.
            StatusCode::TOO_MANY_REQUESTS | StatusCode::SERVICE_UNAVAILABLE => {
                Fetched::Unavailable(SourceError::SlowDown)
            }
            status => Fetched::Unavailable(SourceError::Status {
                status: status.as_u16(),
            }),
        }
    }
}

/// Virtual-host style puts the bucket in the host and leaves it out of the
/// path; path style does the opposite. Getting this pair inconsistent is the
/// single most common cause of a signature that will not verify.
fn signed_host(config: &BucketConfig) -> Result<String, BucketError> {
    let host = config
        .endpoint
        .host_str()
        .ok_or_else(|| BucketError::NoHost {
            endpoint: config.endpoint.as_str().to_string(),
        })?;
    let authority = match config.endpoint.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_string(),
    };
    if config.path_style {
        Ok(authority)
    } else {
        Ok(format!("{}.{authority}", config.bucket))
    }
}

fn canonical_uri(config: &BucketConfig) -> String {
    let key = uri_encode(&config.object_key, false);
    if config.path_style {
        format!("/{}/{key}", uri_encode(&config.bucket, false))
    } else {
        format!("/{key}")
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

async fn body_of(response: reqwest::Response) -> Fetched {
    let etag = response
        .headers()
        .get(header::ETAG)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);

    match response.bytes().await {
        Ok(bytes) => Fetched::Body {
            bytes: bytes.to_vec(),
            etag,
        },
        Err(e) => Fetched::Unavailable(transport_error(&e)),
    }
}

/// `reqwest`'s own message can carry the URL, and a presigned URL is a bearer
/// credential until it expires. Reduce the error to the shape of the failure.
fn transport_error(e: &reqwest::Error) -> SourceError {
    if e.is_timeout() {
        return SourceError::Unreachable("timed out".to_string());
    }
    if e.is_connect() {
        return SourceError::Unreachable("connection refused".to_string());
    }
    SourceError::Unreachable("transport failure".to_string())
}

impl SecretSource for BucketSource {
    fn fetch<'a>(&'a self, etag: Option<&'a str>) -> BoxFuture<'a, Fetched> {
        Box::pin(self.get(etag))
    }

    fn describe(&self) -> String {
        self.description.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(path_style: bool) -> BucketConfig {
        BucketConfig {
            endpoint: "https://s3.example.com".parse().unwrap(),
            bucket: "kallisto".to_string(),
            region: "auto".to_string(),
            object_key: "prod/payment.kal".to_string(),
            path_style,
            access_key_id: "AKIAEXAMPLE".to_string(),
            secret_access_key: "very-secret-key-material".to_string(),
        }
    }

    /// The config holds a bucket credential, and a leaked bucket credential is
    /// how the ADR-0015 D13 attacker gets write access in the first place.
    #[test]
    fn debug_does_not_render_the_credentials() {
        let rendered = format!("{:?}", config(true));
        assert!(!rendered.contains("very-secret-key-material"), "{rendered}");
        assert!(!rendered.contains("AKIAEXAMPLE"), "{rendered}");
        assert!(rendered.contains("s3.example.com"));
    }

    #[test]
    fn path_style_puts_the_bucket_in_the_path_and_not_the_host() {
        let source = BucketSource::new(config(true)).unwrap();
        assert_eq!(source.host, "s3.example.com");
        assert_eq!(source.canonical_uri, "/kallisto/prod/payment.kal");
    }

    #[test]
    fn virtual_host_style_puts_the_bucket_in_the_host_and_not_the_path() {
        let source = BucketSource::new(config(false)).unwrap();
        assert_eq!(source.host, "kallisto.s3.example.com");
        assert_eq!(source.canonical_uri, "/prod/payment.kal");
    }

    /// Garage and MinIO are usually reached on a non-default port, and the port
    /// is part of the signed host. Dropping it is a 403 with no explanation.
    #[test]
    fn a_non_default_port_stays_in_the_signed_host() {
        let mut cfg = config(true);
        cfg.endpoint = "http://localhost:3900".parse().unwrap();
        let source = BucketSource::new(cfg).unwrap();
        assert_eq!(source.host, "localhost:3900");
        assert!(
            source
                .signed_url(0)
                .starts_with("http://localhost:3900/kallisto/")
        );
    }

    #[test]
    fn the_signed_url_carries_the_credential_scope_and_a_signature() {
        let source = BucketSource::new(config(true)).unwrap();
        let url = source.signed_url(1_440_938_160);
        assert!(url.contains("X-Amz-Credential=AKIAEXAMPLE%2F20150830%2Fauto%2Fs3%2Faws4_request"));
        assert!(url.contains("X-Amz-Date=20150830T123600Z"));
        assert!(url.contains("X-Amz-Signature="));
        // The secret itself must never appear in the URL, only a MAC over it.
        assert!(!url.contains("very-secret-key-material"));
    }

    #[test]
    fn describe_names_the_object_without_the_credential() {
        let source = BucketSource::new(config(true)).unwrap();
        assert_eq!(source.describe(), "s3://kallisto/prod/payment.kal");
    }
}
