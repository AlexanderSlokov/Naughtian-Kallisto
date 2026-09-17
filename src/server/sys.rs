//! The endpoints that exist so that Vault clients can start up.
//!
//! None of these serve a secret. They are here because ADR-0015 D7 promises
//! that an unmodified SDK works, and an unmodified SDK asks `sys/health` and
//! `auth/token/lookup-self` before it asks for anything useful. Leaving them
//! out is how a compatibility layer fails on the first line of somebody's
//! `main()`.
//!
//! What changed from the engine era: `sealed` and the version are now *read
//! from the running state*. The old handlers answered `"sealed": false` and
//! `"version": "1.13.0"` as string literals whatever was happening, which made
//! `sys/health` an endpoint that could only ever tell you the process was
//! alive.

use std::time::{SystemTime, UNIX_EPOCH};

use axum::{
    Router,
    extract::State,
    http::StatusCode,
    response::Response,
    routing::{any, get},
};

use super::{
    responses,
    vault_api::{ApiState, json},
};

pub fn router() -> Router<ApiState> {
    Router::new()
        .route("/v1/sys/health", get(health))
        .route("/v1/sys/seal-status", get(seal_status))
        .route("/v1/sys/mounts", get(mounts))
        .route("/v1/sys/internal/ui/mounts/*path", get(ui_mounts))
        .route("/v1/auth/token/lookup-self", any(token_self))
        .route("/v1/auth/token/renew-self", any(token_self))
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// 200 when a file is loaded, 503 when none is — the same two answers Vault
/// gives for unsealed and sealed, so an existing liveness probe keeps working
/// without being told about any of this.
async fn health(State(state): State<ApiState>) -> Response {
    let snapshot = state.snapshot();
    let status = if snapshot.is_some() {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };

    let body = match &snapshot {
        Some(s) => responses::health(
            false,
            now_secs(),
            Some(s.version),
            s.etag.as_deref(),
            Some(s.loaded_at_rfc3339()),
        ),
        None => responses::health(true, now_secs(), None, None, None),
    };
    json(status, body)
}

async fn seal_status(State(state): State<ApiState>) -> Response {
    json(
        StatusCode::OK,
        responses::seal_status(state.snapshot().is_none()),
    )
}

async fn mounts(State(state): State<ApiState>) -> Response {
    json(StatusCode::OK, responses::mounts(&state.resolver.mount))
}

async fn ui_mounts(State(state): State<ApiState>) -> Response {
    json(StatusCode::OK, responses::ui_mount(&state.resolver.mount))
}

/// `lookup-self` and `renew-self` answer the same thing, because nothing here
/// expires: the token is a key into a table in the sealed file, and the file
/// is the only thing that changes.
///
/// Until M4 lands there is no token table consulted here, so this reports the
/// `default` policy for anyone who asks. That is not an authorisation
/// decision — no read path consults this answer — but it does mean the
/// endpoint currently tells a client less than it will.
async fn token_self() -> Response {
    json(
        StatusCode::OK,
        responses::token_lookup(&["default".to_string()]),
    )
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, sync::Arc};

    use axum::{body::Body, http::Request};
    use core_crypto::Contents;
    use tower::ServiceExt;

    use super::*;
    use crate::{
        config::ResolvedLimits,
        resolver::snapshot::{Snapshot, SnapshotSlot},
        server::vault_api::{Resolver, router as api_router},
    };

    fn app(loaded: bool) -> Router {
        let slot = Arc::new(SnapshotSlot::empty());
        if loaded {
            slot.store(Snapshot::build(
                Contents {
                    version: 42,
                    secrets: BTreeMap::from([("a".to_string(), serde_json::json!({"k": "v"}))]),
                    policies: BTreeMap::new(),
                    tokens: BTreeMap::new(),
                },
                Some("\"tag-42\"".to_string()),
            ));
        }
        api_router(Resolver {
            slot,
            mount: "secret".into(),
            limits: ResolvedLimits {
                requests_per_second: 10_000,
                burst: 10_000,
            },
        })
    }

    async fn get_json(loaded: bool, uri: &str) -> (StatusCode, serde_json::Value) {
        let response = app(loaded)
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    /// The whole point of rewriting this file: the answer used to be a
    /// constant.
    #[tokio::test]
    async fn health_reports_the_file_it_is_actually_serving() {
        let (status, body) = get_json(true, "/v1/sys/health").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["sealed"], false);
        assert_eq!(body["kallisto_file_version"], 42);
        assert_eq!(body["kallisto_etag"], "\"tag-42\"");
        assert!(body["kallisto_loaded_at"].is_string());
    }

    #[tokio::test]
    async fn health_is_503_and_sealed_when_no_file_has_loaded() {
        let (status, body) = get_json(false, "/v1/sys/health").await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body["sealed"], true);
        assert!(body["kallisto_file_version"].is_null());
    }

    #[tokio::test]
    async fn seal_status_answers_200_either_way_and_tells_the_truth() {
        let (status, body) = get_json(false, "/v1/sys/seal-status").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["sealed"], true);

        let (_, body) = get_json(true, "/v1/sys/seal-status").await;
        assert_eq!(body["sealed"], false);
    }

    #[tokio::test]
    async fn the_mount_listing_matches_the_configured_mount() {
        let (status, body) = get_json(true, "/v1/sys/mounts").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["data"]["secret/"]["options"]["version"], "2");
    }

    #[tokio::test]
    async fn the_startup_calls_an_sdk_makes_all_answer() {
        for uri in [
            "/v1/sys/health",
            "/v1/sys/seal-status",
            "/v1/sys/mounts",
            "/v1/sys/internal/ui/mounts/secret",
            "/v1/auth/token/lookup-self",
        ] {
            let (status, _) = get_json(true, uri).await;
            assert_eq!(status, StatusCode::OK, "{uri}");
        }
    }
}
