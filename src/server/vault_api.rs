//! The read-only Vault KV-v2 surface (ADR-0015 D1, D7).
//!
//! This is where the output port was welded shut (ADR-0016 QĐ-4). There is no
//! `SecretEngine`, no registry, no `Arc<dyn>`: a handler loads the current
//! [`Snapshot`] and reads a `HashMap`. The abstraction that used to sit here
//! bought a substitutability nobody asked for and charged a virtual call per
//! request for it.
//!
//! Everything that writes answers 403. That is not an unimplemented feature —
//! it is the product. A resolver that cannot write is a resolver whose
//! credentials are worth nothing to an attacker who steals them.

use std::sync::Arc;

use axum::{
    Router,
    extract::State,
    http::{Method, StatusCode, Uri, header},
    response::{IntoResponse, Response},
    routing::any,
};

use super::{rate_limit::RateLimiter, responses, sys};
use crate::{
    config::ResolvedLimits,
    resolver::snapshot::{Snapshot, SnapshotSlot},
};

/// What every worker shares: the table, the mount name, and the shape of the
/// limiter each worker will build for itself.
#[derive(Clone)]
pub struct Resolver {
    pub slot: Arc<SnapshotSlot>,
    pub mount: Arc<str>,
    pub limits: ResolvedLimits,
}

/// What one worker's router holds. The limiter is created inside [`router`],
/// which runs once per worker, so each worker counts its own traffic against
/// its own bucket (ADR-0016 QĐ-3).
#[derive(Clone)]
pub struct ApiState {
    pub resolver: Resolver,
    pub limiter: Arc<RateLimiter>,
}

impl ApiState {
    pub fn snapshot(&self) -> Option<Arc<Snapshot>> {
        self.resolver.slot.load()
    }
}

pub fn router(resolver: Resolver) -> Router {
    let limiter = Arc::new(RateLimiter::new(
        resolver.limits.requests_per_second,
        resolver.limits.burst,
    ));
    let state = ApiState { resolver, limiter };

    Router::new()
        .route("/v1/:mount/data/*path", any(data))
        .route("/v1/:mount/metadata/*path", any(metadata))
        // Present so they answer 403 rather than 404. A 404 would read as "not
        // deployed yet"; 403 says the door exists and is shut.
        .route("/v1/:mount/subkeys/*path", any(read_only))
        .route("/v1/:mount/delete/*path", any(read_only))
        .route("/v1/:mount/undelete/*path", any(read_only))
        .route("/v1/:mount/destroy/*path", any(read_only))
        .merge(sys::router())
        .fallback(no_handler)
        .with_state(state)
}

// -----------------------------------------------------------------------------
// Shared shapes
// -----------------------------------------------------------------------------

pub fn json(status: StatusCode, body: String) -> Response {
    (status, [(header::CONTENT_TYPE, "application/json")], body).into_response()
}

/// Vault's own wording. Clients match on it, so it is compatibility surface
/// rather than a message we are free to improve.
pub const SEALED_MESSAGE: &str = "Vault is sealed";
const DENIED_MESSAGE: &str = "permission denied";

pub fn sealed() -> Response {
    json(
        StatusCode::SERVICE_UNAVAILABLE,
        responses::errors(SEALED_MESSAGE),
    )
}

fn denied() -> Response {
    json(StatusCode::FORBIDDEN, responses::errors(DENIED_MESSAGE))
}

/// Vault answers a missing KV-v2 secret with a 404 and an empty error list.
fn absent() -> Response {
    json(StatusCode::NOT_FOUND, r#"{"errors":[]}"#.to_string())
}

fn too_many(retry_after_secs: u64) -> Response {
    (
        StatusCode::TOO_MANY_REQUESTS,
        [
            (header::CONTENT_TYPE, "application/json"),
            (header::RETRY_AFTER, &retry_after_secs.to_string()),
        ],
        responses::errors("request rate exceeded"),
    )
        .into_response()
}

async fn no_handler(uri: Uri) -> Response {
    route_not_found(uri.path())
}

fn route_not_found(path: &str) -> Response {
    let route = path.strip_prefix("/v1/").unwrap_or(path);
    json(
        StatusCode::NOT_FOUND,
        responses::errors(&format!(
            "no handler for route \"{route}\". route entry not found."
        )),
    )
}

/// The gate every serving handler passes through: rate limit first, because a
/// refused request should cost as little as possible, then the sealed check.
fn admit(state: &ApiState) -> Result<Arc<Snapshot>, Response> {
    if let Err(retry_after) = state.limiter.try_acquire() {
        return Err(too_many(retry_after));
    }
    state.snapshot().ok_or_else(sealed)
}

/// `/v1/{mount}/{action}/{path}` split into its three parts.
///
/// Carried over unchanged from the engine-era handler, tests included: it was
/// correct, it is on the hot path, and rewriting working string slicing is how
/// a migration acquires bugs it did not have before.
fn extract_mount_and_path<'a>(
    uri_path: &'a str,
    expected_action: &str,
) -> Option<(&'a str, &'a str)> {
    let path_without_version = uri_path.strip_prefix("/v1/")?;
    let mut path_segments = path_without_version.splitn(3, '/');

    let mount = path_segments.next()?;
    let action = path_segments.next()?;

    if action != expected_action {
        return None;
    }

    let secret_path = path_segments.next()?;
    Some((mount, secret_path))
}

/// `?version=N`, absent or unparseable meaning "whatever is current".
#[inline]
fn version_param(uri: &Uri) -> Option<u64> {
    let query = uri.query()?;
    let start = query.find("version=")? + "version=".len();
    let rest = &query[start..];
    let end = rest.find('&').unwrap_or(rest.len());
    rest[..end].parse::<u64>().ok()
}

#[inline]
fn wants_list(uri: &Uri) -> bool {
    uri.query().is_some_and(|q| q.contains("list=true"))
}

/// Vault accepts the made-up `LIST` verb as well as `GET ?list=true`, and the
/// SDKs are split on which they send: the Go client sends `LIST`, several
/// others send the query parameter. Supporting only one of them passes every
/// unit test and fails `vault kv list`.
#[inline]
fn is_list_method(method: &Method) -> bool {
    method.as_str().eq_ignore_ascii_case("LIST")
}

#[inline]
fn is_read_method(method: &Method) -> bool {
    method == Method::GET || method == Method::HEAD
}

// -----------------------------------------------------------------------------
// Handlers
// -----------------------------------------------------------------------------

async fn data(State(state): State<ApiState>, method: Method, uri: Uri) -> Response {
    if !is_read_method(&method) {
        return denied();
    }
    let snapshot = match admit(&state) {
        Ok(s) => s,
        Err(response) => return response,
    };

    let Some((mount, path)) = extract_mount_and_path(uri.path(), "data") else {
        return route_not_found(uri.path());
    };
    if mount != &*state.resolver.mount {
        return route_not_found(uri.path());
    }

    // ADR-0016 QĐ-2: the only version this process has is the one it is
    // serving. Asking for any other is a 404, not a lie.
    if let Some(asked) = version_param(&uri)
        && asked != 0
        && asked != snapshot.version
    {
        return absent();
    }

    match snapshot.secret(path) {
        Some(value) => json(
            StatusCode::OK,
            responses::kv_data(value, snapshot.version, snapshot.loaded_at_rfc3339()),
        ),
        None => absent(),
    }
}

async fn metadata(State(state): State<ApiState>, method: Method, uri: Uri) -> Response {
    let listing = is_list_method(&method) || (is_read_method(&method) && wants_list(&uri));
    if !listing && !is_read_method(&method) {
        return denied();
    }

    let snapshot = match admit(&state) {
        Ok(s) => s,
        Err(response) => return response,
    };

    let Some((mount, path)) = extract_mount_and_path(uri.path(), "metadata") else {
        return route_not_found(uri.path());
    };
    if mount != &*state.resolver.mount {
        return route_not_found(uri.path());
    }

    if listing {
        // Vault treats `a` and `a/` as the same directory on a LIST.
        let prefix = if path.is_empty() || path.ends_with('/') {
            path.to_string()
        } else {
            format!("{path}/")
        };
        let keys = snapshot.children(&prefix);
        if keys.is_empty() {
            return absent();
        }
        return json(StatusCode::OK, responses::list_keys(&keys));
    }

    if snapshot.secret(path).is_none() {
        return absent();
    }
    json(
        StatusCode::OK,
        responses::kv_metadata(snapshot.version, snapshot.loaded_at_rfc3339()),
    )
}

/// Every mutating corner of the KV-v2 API, in one answer.
async fn read_only(State(state): State<ApiState>) -> Response {
    // Still costs a permit: an attacker hammering `destroy` should meet the
    // same limiter as everyone else.
    if let Err(retry_after) = state.limiter.try_acquire() {
        return too_many(retry_after);
    }
    denied()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use axum::{body::Body, http::Request};
    use core_crypto::Contents;
    use tower::ServiceExt;

    use super::*;

    fn contents(version: u64) -> Contents {
        Contents {
            version,
            secrets: BTreeMap::from([
                (
                    "app/db".to_string(),
                    serde_json::json!({"user": "admin", "password": "hunter2"}),
                ),
                (
                    "app/web".to_string(),
                    serde_json::json!({"url": "http://x"}),
                ),
                ("app/sub/deep".to_string(), serde_json::json!({"k": "v"})),
            ]),
            policies: BTreeMap::new(),
            tokens: BTreeMap::new(),
        }
    }

    fn app_with(snapshot: Option<Snapshot>) -> Router {
        let slot = Arc::new(SnapshotSlot::empty());
        if let Some(s) = snapshot {
            slot.store(s);
        }
        router(Resolver {
            slot,
            mount: "secret".into(),
            limits: ResolvedLimits {
                requests_per_second: 100_000,
                burst: 100_000,
            },
        })
    }

    fn loaded() -> Router {
        app_with(Some(Snapshot::build(contents(12), Some("\"etag\"".into()))))
    }

    async fn send(app: Router, method: &str, uri: &str) -> (StatusCode, serde_json::Value) {
        let response = app
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(uri)
                    .body(Body::empty())
                    .unwrap(),
            )
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

    #[tokio::test]
    async fn a_secret_reads_back_in_vaults_shape() {
        let (status, body) = send(loaded(), "GET", "/v1/secret/data/app/db").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["data"]["data"]["password"], "hunter2");
        assert_eq!(body["data"]["metadata"]["version"], 12);
    }

    #[tokio::test]
    async fn a_missing_secret_is_vaults_404() {
        let (status, body) = send(loaded(), "GET", "/v1/secret/data/app/nope").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body["errors"].as_array().unwrap().len(), 0);
    }

    /// ADR-0016 QĐ-2 made this a visible limitation rather than a silent one:
    /// the current version answers, any other number 404s.
    #[tokio::test]
    async fn asking_for_a_version_we_do_not_have_is_a_404() {
        let (ok, _) = send(loaded(), "GET", "/v1/secret/data/app/db?version=12").await;
        assert_eq!(ok, StatusCode::OK);
        let (current, _) = send(loaded(), "GET", "/v1/secret/data/app/db?version=0").await;
        assert_eq!(current, StatusCode::OK);
        let (old, _) = send(loaded(), "GET", "/v1/secret/data/app/db?version=11").await;
        assert_eq!(old, StatusCode::NOT_FOUND);
    }

    /// The trap the plan flagged: miss either spelling and `vault kv list`
    /// fails while every read test stays green.
    #[tokio::test]
    async fn list_works_through_both_spellings() {
        for (method, uri) in [
            ("LIST", "/v1/secret/metadata/app"),
            ("GET", "/v1/secret/metadata/app?list=true"),
            ("LIST", "/v1/secret/metadata/app/"),
        ] {
            let (status, body) = send(loaded(), method, uri).await;
            assert_eq!(status, StatusCode::OK, "{method} {uri}");
            let keys: Vec<String> = serde_json::from_value(body["data"]["keys"].clone()).unwrap();
            assert_eq!(keys, vec!["db", "sub/", "web"], "{method} {uri}");
        }
    }

    #[tokio::test]
    async fn listing_an_empty_prefix_is_a_404_as_in_vault() {
        let (status, _) = send(loaded(), "LIST", "/v1/secret/metadata/nothing").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn metadata_reads_without_the_list_flag() {
        let (status, body) = send(loaded(), "GET", "/v1/secret/metadata/app/db").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["data"]["current_version"], 12);
    }

    /// ADR-0015 D1. Every one of these used to succeed.
    #[tokio::test]
    async fn every_way_of_writing_is_refused() {
        for (method, uri) in [
            ("PUT", "/v1/secret/data/app/db"),
            ("POST", "/v1/secret/data/app/db"),
            ("PATCH", "/v1/secret/data/app/db"),
            ("DELETE", "/v1/secret/data/app/db"),
            ("POST", "/v1/secret/delete/app/db"),
            ("POST", "/v1/secret/undelete/app/db"),
            ("PUT", "/v1/secret/destroy/app/db"),
            ("GET", "/v1/secret/subkeys/app/db"),
            ("PUT", "/v1/secret/metadata/app/db"),
            ("DELETE", "/v1/secret/metadata/app/db"),
        ] {
            let (status, body) = send(loaded(), method, uri).await;
            assert_eq!(status, StatusCode::FORBIDDEN, "{method} {uri} was allowed");
            assert_eq!(body["errors"][0], "permission denied", "{method} {uri}");
        }
    }

    /// A write that is refused must not tell the caller whether the secret is
    /// there — 403 before any lookup, for a path that does not exist either.
    #[tokio::test]
    async fn a_refused_write_reveals_nothing_about_what_exists() {
        let (real, _) = send(loaded(), "PUT", "/v1/secret/data/app/db").await;
        let (fake, _) = send(loaded(), "PUT", "/v1/secret/data/does/not/exist").await;
        assert_eq!(real, fake);
    }

    /// ADR-0015 D14: no table yet is Vault's sealed state, not an empty 200.
    #[tokio::test]
    async fn an_unloaded_resolver_answers_503_not_an_empty_success() {
        let (status, body) = send(app_with(None), "GET", "/v1/secret/data/app/db").await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body["errors"][0], SEALED_MESSAGE);
    }

    #[tokio::test]
    async fn another_mount_is_not_ours_to_answer_for() {
        let (status, body) = send(loaded(), "GET", "/v1/kv/data/app/db").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert!(
            body["errors"][0]
                .as_str()
                .unwrap()
                .contains("no handler for route")
        );
    }

    #[tokio::test]
    async fn an_unknown_route_answers_the_way_vault_does() {
        let (status, body) = send(loaded(), "GET", "/v1/nonsense").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert!(
            body["errors"][0]
                .as_str()
                .unwrap()
                .contains("route entry not found")
        );
    }

    #[tokio::test]
    async fn over_the_limit_is_a_429_with_a_retry_after() {
        let slot = Arc::new(SnapshotSlot::empty());
        slot.store(Snapshot::build(contents(1), None));
        let app = router(Resolver {
            slot,
            mount: "secret".into(),
            limits: ResolvedLimits {
                requests_per_second: 1,
                burst: 1,
            },
        });

        let (first, _) = send(app.clone(), "GET", "/v1/secret/data/app/db").await;
        assert_eq!(first, StatusCode::OK);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/v1/secret/data/app/db")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        let retry = response.headers().get(header::RETRY_AFTER).unwrap();
        assert!(retry.to_str().unwrap().parse::<u64>().unwrap() >= 1);
    }

    #[test]
    fn the_uri_splitter_still_behaves() {
        assert_eq!(
            extract_mount_and_path("/v1/secret/data/a/b", "data"),
            Some(("secret", "a/b"))
        );
        assert_eq!(
            extract_mount_and_path("/v1/secret/data/a", "metadata"),
            None
        );
        assert_eq!(extract_mount_and_path("/v1/secret/data", "data"), None);
        assert_eq!(extract_mount_and_path("/secret/data/a", "data"), None);
    }

    #[test]
    fn the_version_parameter_is_read_without_a_parser() {
        assert_eq!(version_param(&"/x?version=7".parse().unwrap()), Some(7));
        assert_eq!(
            version_param(&"/x?a=1&version=7&b=2".parse().unwrap()),
            Some(7)
        );
        assert_eq!(version_param(&"/x?version=abc".parse().unwrap()), None);
        assert_eq!(version_param(&"/x".parse().unwrap()), None);
    }
}
