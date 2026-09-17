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
    http::{HeaderMap, StatusCode},
    response::Response,
    routing::{any, get},
};

use super::{
    responses,
    vault_api::{ApiState, denied, json, presented_token, sealed},
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
            Some(s.enforces()),
        ),
        None => responses::health(true, now_secs(), None, None, None, None),
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
/// expires: the token is a key into a table in the sealed file, and the file is
/// the only thing that changes. `renew-self` therefore renews nothing, and says
/// so by reporting a zero TTL, which is what a non-renewable token looks like
/// to a client that knows how to read the answer.
///
/// A file with no token table reports `root`: that is Vault's word for a token
/// with no restrictions, and with no table there are none.
async fn token_self(State(state): State<ApiState>, headers: HeaderMap) -> Response {
    let Some(snapshot) = state.snapshot() else {
        return sealed();
    };
    if !snapshot.enforces() {
        return json(
            StatusCode::OK,
            responses::token_lookup(&["root".to_string()]),
        );
    }
    match snapshot.grant(presented_token(&headers)) {
        Some(grant) => json(StatusCode::OK, responses::token_lookup(grant.policies)),
        None => denied(),
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, sync::Arc};

    use axum::{body::Body, http::Request};
    use core_crypto::{Contents, PolicyRule};
    use policy_engine::TokenKey;
    use tower::ServiceExt;

    use super::*;
    use crate::{
        config::ResolvedLimits,
        resolver::snapshot::SnapshotSlot,
        server::vault_api::{Resolver, router as api_router},
    };

    const TOKEN: &str = "s.apptoken";

    fn token_key() -> TokenKey {
        TokenKey::from_bytes([5u8; 32])
    }

    fn contents(guarded: bool) -> Contents {
        let key = token_key();
        Contents {
            version: 42,
            secrets: BTreeMap::from([("a".to_string(), serde_json::json!({"k": "v"}))]),
            policies: if guarded {
                BTreeMap::from([(
                    "reader".to_string(),
                    vec![PolicyRule {
                        path: "secret/data/*".to_string(),
                        capabilities: vec!["read".to_string()],
                    }],
                )])
            } else {
                BTreeMap::new()
            },
            tokens: if guarded {
                BTreeMap::from([(key.hash_hex(TOKEN), vec!["reader".to_string()])])
            } else {
                BTreeMap::new()
            },
            token_key: guarded.then(|| key.expose_as_hex()),
        }
    }

    fn app(loaded: bool) -> Router {
        app_of(loaded, false)
    }

    fn app_of(loaded: bool, guarded: bool) -> Router {
        let slot = Arc::new(SnapshotSlot::empty());
        if loaded {
            slot.store(
                crate::resolver::snapshot::from_contents(
                    &contents(guarded),
                    Some("\"tag-42\"".to_string()),
                )
                .unwrap(),
            );
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
        send(app(loaded), uri, None).await
    }

    async fn send(app: Router, uri: &str, token: Option<&str>) -> (StatusCode, serde_json::Value) {
        let mut request = Request::builder().uri(uri);
        if let Some(token) = token {
            request = request.header("x-vault-token", token);
        }
        let response = app
            .oneshot(request.body(Body::empty()).unwrap())
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

    /// A file with no token table serves everything to everyone, which is a
    /// legitimate deployment and a bad surprise. `sys/health` says which one
    /// this machine is in.
    #[tokio::test]
    async fn health_says_whether_authorization_is_being_enforced() {
        let (_, open) = send(app_of(true, false), "/v1/sys/health", None).await;
        assert_eq!(open["kallisto_authorization"], "none");

        let (_, guarded) = send(app_of(true, true), "/v1/sys/health", None).await;
        assert_eq!(guarded["kallisto_authorization"], "enforced");
    }

    /// Several SDKs call this before anything else and give up if it fails.
    #[tokio::test]
    async fn lookup_self_reports_the_policies_the_token_actually_carries() {
        let (status, body) = send(
            app_of(true, true),
            "/v1/auth/token/lookup-self",
            Some(TOKEN),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let policies: Vec<String> =
            serde_json::from_value(body["data"]["policies"].clone()).unwrap();
        assert_eq!(policies, vec!["reader".to_string()]);
    }

    #[tokio::test]
    async fn lookup_self_refuses_a_token_the_file_does_not_carry() {
        let (status, _) = send(
            app_of(true, true),
            "/v1/auth/token/lookup-self",
            Some("s.nope"),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);

        let (missing, _) = send(app_of(true, true), "/v1/auth/token/lookup-self", None).await;
        assert_eq!(missing, StatusCode::FORBIDDEN);
    }

    /// With no table there are no restrictions, and `root` is Vault's word for
    /// that.
    #[tokio::test]
    async fn lookup_self_reports_root_when_no_table_exists() {
        let (status, body) = send(app_of(true, false), "/v1/auth/token/lookup-self", None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["data"]["policies"][0], "root");
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
