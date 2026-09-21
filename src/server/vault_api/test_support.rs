//! Fixtures shared by the handler tests and the access-log tests.
//!
//! A sealed file is expensive to hand-build and easy to get subtly wrong, so
//! both suites take theirs from here rather than each growing its own.

use std::{collections::BTreeMap, sync::Arc};

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use core_crypto::{Contents, PolicyRule};
use policy_engine::TokenKey;
use tower::ServiceExt;

use super::{Resolver, Telemetry, replies::TOKEN_HEADER, router};
use crate::{
    config::ResolvedLimits,
    resolver::snapshot::{Snapshot, SnapshotSlot},
};

pub(crate) const TOKEN: &str = "s.apptoken";
pub(crate) const DENIED_TOKEN: &str = "s.deniedtoken";

pub(crate) fn token_key() -> TokenKey {
    TokenKey::from_bytes([3u8; 32])
}

fn rule(path: &str, capabilities: &[&str]) -> PolicyRule {
    PolicyRule {
        path: path.to_string(),
        capabilities: capabilities.iter().map(|c| (*c).to_string()).collect(),
    }
}

pub(crate) fn contents(version: u64) -> Contents {
    Contents {
        version,
        secrets: BTreeMap::from([
            (
                "app/db".to_string(),
                serde_json::json!({"user": "admin", "password": "duck-fixture-not-a-credential"}),
            ),
            (
                "app/web".to_string(),
                serde_json::json!({"url": "http://x"}),
            ),
            ("app/sub/deep".to_string(), serde_json::json!({"k": "v"})),
        ]),
        policies: BTreeMap::new(),
        tokens: BTreeMap::new(),
        token_key: None,
    }
}

/// The same secrets, but with a token table: one token that may read and
/// list under `app/`, and one that is explicitly denied `app/db`.
pub(crate) fn guarded_contents() -> Contents {
    let key = token_key();
    Contents {
        policies: BTreeMap::from([
            (
                "app".to_string(),
                vec![
                    rule("secret/data/app/*", &["read"]),
                    rule("secret/metadata/app/*", &["read", "list"]),
                ],
            ),
            (
                "app-minus-db".to_string(),
                vec![
                    rule("secret/data/app/*", &["read"]),
                    rule("secret/data/app/db", &["deny"]),
                ],
            ),
        ]),
        tokens: BTreeMap::from([
            (key.hash_hex(TOKEN), vec!["app".to_string()]),
            (key.hash_hex(DENIED_TOKEN), vec!["app-minus-db".to_string()]),
        ]),
        token_key: Some(key.expose_as_hex()),
        ..contents(12)
    }
}

fn snapshot_of(contents: &Contents, etag: Option<String>) -> Snapshot {
    crate::resolver::snapshot::from_contents(contents, etag).unwrap()
}

/// A router whose limiter is wide enough never to interfere. Tests that are
/// *about* the limiter build their own with [`throttled`].
pub(crate) fn app_with(snapshot: Option<Snapshot>) -> Router {
    observed_app_with(snapshot, Arc::new(Telemetry::new(64, 1)))
}

/// Same router, but the caller keeps the [`Telemetry`] so it can read back
/// what the request produced on the log and the counters.
pub(crate) fn observed_app_with(snapshot: Option<Snapshot>, telemetry: Arc<Telemetry>) -> Router {
    router(resolver_with(snapshot, telemetry, 100_000))
}

/// A router that admits `per_second` requests and no more, for the tests that
/// are about refusal rather than about serving.
pub(crate) fn throttled(contents: &Contents) -> Router {
    router(resolver_with(
        Some(snapshot_of(contents, None)),
        Arc::new(Telemetry::new(64, 1)),
        1,
    ))
}

fn resolver_with(
    snapshot: Option<Snapshot>,
    telemetry: Arc<Telemetry>,
    per_second: u64,
) -> Resolver {
    let slot = Arc::new(SnapshotSlot::empty());
    if let Some(s) = snapshot {
        slot.store(s);
    }
    Resolver {
        slot,
        mount: "secret".into(),
        telemetry,
        limits: ResolvedLimits {
            requests_per_second: per_second,
            burst: per_second,
        },
    }
}

/// The guarded fixture, with the caller holding the telemetry.
pub(crate) fn observed(telemetry: Arc<Telemetry>) -> Router {
    observed_app_with(Some(snapshot_of(&guarded_contents(), None)), telemetry)
}

/// The plain fixture: three secrets, no token table.
pub(crate) fn loaded() -> Router {
    app_with(Some(snapshot_of(&contents(12), Some("\"etag\"".into()))))
}

/// The fixture with a token table, for the authorization tests.
pub(crate) fn guarded() -> Router {
    app_with(Some(snapshot_of(&guarded_contents(), None)))
}

pub(crate) async fn send(app: Router, method: &str, uri: &str) -> (StatusCode, serde_json::Value) {
    send_as(app, method, uri, None).await
}

pub(crate) async fn send_as(
    app: Router,
    method: &str,
    uri: &str,
    token: Option<&str>,
) -> (StatusCode, serde_json::Value) {
    let mut request = Request::builder().method(method).uri(uri);
    if let Some(token) = token {
        request = request.header(TOKEN_HEADER, token);
    }
    let response = app
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body = if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes)
            .unwrap_or_else(|e| panic!("not JSON: {e}\n{}", String::from_utf8_lossy(&bytes)))
    };
    (status, body)
}
