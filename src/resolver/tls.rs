//! The TLS configuration for the bucket fetch, on the same crypto library as
//! the barrier (issue #58).
//!
//! Until 2.0.2 `reqwest` was built with `rustls-tls`, which in reqwest 0.12
//! selects the `ring` provider. The release binary therefore carried two crypto
//! libraries, `ring` for TLS and `aws-lc-rs` for the barrier (ADR-0015 D13) and
//! keyed hashing (D15), while two comments said it carried one.
//!
//! The provider is passed in explicitly instead of installed as rustls's
//! process-wide default. A default is global state: whichever crate installs
//! one first wins, and a later dependency could put a different library under
//! the bucket fetch without a line changing here.
//!
//! What this does not give: FIPS. AWS-LC has a FIPS 140-3 validated build, but
//! it is a different one (`aws-lc-fips-sys`, behind rustls's `fips` feature),
//! and Kallisto does not build it.

use std::sync::Arc;

use rustls::{ClientConfig, RootCertStore, crypto::CryptoProvider};

#[derive(Debug, thiserror::Error)]
#[error("the TLS provider cannot offer rustls's default protocol versions: {0}")]
pub struct TlsConfigError(String);

/// The crypto library every bucket connection runs on.
pub fn bucket_tls_provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::aws_lc_rs::default_provider())
}

/// A client configuration that trusts Mozilla's roots and presents no client
/// certificate. A bucket is authenticated to with a signed URL, not with mTLS.
///
/// ```
/// use naughtian_kallisto::resolver::tls::{bucket_tls_config, bucket_tls_provider};
///
/// let tls = bucket_tls_config(bucket_tls_provider()).unwrap();
/// assert!(!tls.crypto_provider().cipher_suites.is_empty());
/// ```
pub fn bucket_tls_config(provider: Arc<CryptoProvider>) -> Result<ClientConfig, TlsConfigError> {
    let roots = RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let builder = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| TlsConfigError(e.to_string()))?;
    Ok(builder.with_root_certificates(roots).with_no_client_auth())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A provider with no cipher suites cannot negotiate any protocol version,
    /// and that must be an error at start-up, not a failed handshake at the
    /// first poll.
    #[test]
    fn a_provider_with_no_cipher_suites_is_refused() {
        let mut empty = rustls::crypto::aws_lc_rs::default_provider();
        empty.cipher_suites.clear();
        let err = bucket_tls_config(Arc::new(empty)).unwrap_err();
        assert!(err.to_string().contains("protocol versions"), "{err}");
    }

    /// Regression test for #58, as far as a unit test can see it: the
    /// configuration runs on the provider passed in, not on whatever a feature
    /// default would have picked. That `ring` is absent from the build is a
    /// property of the dependency graph, and `make deny` checks it. Cargo.lock
    /// cannot be asked: it lists `ring` as an optional dependency of rustls
    /// whether or not anything enables it.
    #[test]
    fn the_bucket_configuration_keeps_the_provider_it_was_given() {
        let provider = bucket_tls_provider();
        let tls = bucket_tls_config(Arc::clone(&provider)).unwrap();
        assert!(Arc::ptr_eq(tls.crypto_provider(), &provider));
    }
}
