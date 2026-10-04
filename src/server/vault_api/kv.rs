//! The three handlers behind `/v1/{mount}/…`: read a secret, read or list its
//! metadata, and refuse everything else.

use std::sync::Arc;

use axum::{
    Router,
    extract::State,
    http::{HeaderMap, Method, StatusCode, Uri},
    response::Response,
    routing::any,
};
use policy_engine::Capability;

use super::{
    ApiState,
    replies::{absent, denied, json_response, route_not_found, too_many},
    request::{
        extract_mount_and_path, is_list_method, is_read_method, policy_path, presented_token,
        version_param, wants_list,
    },
};
use crate::{resolver::snapshot::Snapshot, server::responses};

pub(super) fn routes() -> Router<ApiState> {
    Router::new()
        .route("/v1/:mount/data/*path", any(read_secret))
        .route("/v1/:mount/metadata/*path", any(read_metadata))
        // Present so they answer 403 rather than 404. A 404 would read as "not
        // deployed yet"; 403 says the door exists and is shut.
        .route("/v1/:mount/subkeys/*path", any(refuse_write))
        .route("/v1/:mount/delete/*path", any(refuse_write))
        .route("/v1/:mount/undelete/*path", any(refuse_write))
        .route("/v1/:mount/destroy/*path", any(refuse_write))
}

/// The gate every serving handler passes through, in this order: rate limit
/// first, because a refused request should cost as little as possible, then the
/// sealed check, then the route and the mount.
///
/// Returns the snapshot to serve from and the secret path the URI named.
fn accept<'u>(
    state: &ApiState,
    uri: &'u Uri,
    action: &str,
) -> Result<(Arc<Snapshot>, &'u str), Response> {
    if let Err(retry_after) = state.limiter.try_acquire() {
        return Err(too_many(retry_after));
    }
    let snapshot = state.snapshot().ok_or_else(super::replies::sealed)?;

    let Some((mount, path)) = extract_mount_and_path(uri.path(), action) else {
        return Err(route_not_found(uri.path()));
    };
    if mount != &*state.resolver.mount {
        return Err(route_not_found(uri.path()));
    }
    Ok((snapshot, path))
}

async fn read_secret(
    State(state): State<ApiState>,
    method: Method,
    headers: HeaderMap,
    uri: Uri,
) -> Response {
    if !is_read_method(&method) {
        return denied();
    }
    let (snapshot, path) = match accept(&state, &uri, "data") {
        Ok(admitted) => admitted,
        Err(response) => return response,
    };
    if let Some(refusal) = read_refusal(&snapshot, &headers, &uri) {
        return refusal;
    }
    render_secret(&snapshot, path)
}

/// Both refusals a read can meet once the route has matched.
///
/// Authorization comes first, so that a refusal says nothing about whether the
/// secret exists.
fn read_refusal(snapshot: &Snapshot, headers: &HeaderMap, uri: &Uri) -> Option<Response> {
    if !snapshot.permits(presented_token(headers), policy_path(uri), Capability::Read) {
        return Some(denied());
    }
    // ADR-0016 QĐ-2: the only version this process has is the one it is
    // serving. Asking for any other is a 404, not a lie.
    if let Some(asked) = version_param(uri)
        && asked != 0
        && asked != snapshot.version
    {
        return Some(absent());
    }
    None
}

/// The response body is built *inside* the barrier's callback, so the cleartext
/// exists in this worker's buffer for exactly as long as it takes to copy it
/// into the body and no longer (ADR-0015 D13).
fn render_secret(snapshot: &Snapshot, path: &str) -> Response {
    match snapshot.with_secret(path, |value| {
        responses::kv_data(value, snapshot.version, snapshot.loaded_at_rfc3339())
    }) {
        Some(Ok(body)) => json_response(StatusCode::OK, body),
        // The barrier could not open ciphertext it produced itself. That is
        // memory corruption, not anything the request did, and it must not read
        // as "no such secret".
        Some(Err(_)) => json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            responses::errors("internal error"),
        ),
        None => absent(),
    }
}

async fn read_metadata(
    State(state): State<ApiState>,
    method: Method,
    headers: HeaderMap,
    uri: Uri,
) -> Response {
    let listing = is_list_method(&method) || (is_read_method(&method) && wants_list(&uri));
    if !listing && !is_read_method(&method) {
        return denied();
    }
    let (snapshot, path) = match accept(&state, &uri, "metadata") {
        Ok(admitted) => admitted,
        Err(response) => return response,
    };

    let token = presented_token(&headers);
    if listing {
        return list_children(&snapshot, &state.resolver.mount, token, path);
    }
    describe_secret(&snapshot, token, &uri, path)
}

/// Vault treats `a` and `a/` as the same directory on a LIST, and a policy
/// written `secret/metadata/app/*` is expected to cover listing `app` itself.
/// Normalising to the trailing-slash form is what makes the policy an operator
/// copied from Vault behave as it does there.
fn list_children(snapshot: &Snapshot, mount: &str, token: Option<&str>, path: &str) -> Response {
    let prefix = if path.is_empty() || path.ends_with('/') {
        path.to_string()
    } else {
        format!("{path}/")
    };
    if !snapshot.permits(
        token,
        &format!("{mount}/metadata/{prefix}"),
        Capability::List,
    ) {
        return denied();
    }

    let keys = snapshot.children(&prefix);
    if keys.is_empty() {
        return absent();
    }
    json_response(StatusCode::OK, responses::list_keys(&keys))
}

/// Metadata carries no secret value, so this answers without opening anything.
fn describe_secret(snapshot: &Snapshot, token: Option<&str>, uri: &Uri, path: &str) -> Response {
    if !snapshot.permits(token, policy_path(uri), Capability::Read) {
        return denied();
    }
    if !snapshot.has_secret(path) {
        return absent();
    }
    json_response(
        StatusCode::OK,
        responses::kv_metadata(snapshot.version, snapshot.loaded_at_rfc3339()),
    )
}

/// Every mutating corner of the KV-v2 API, in one answer.
async fn refuse_write(State(state): State<ApiState>) -> Response {
    // Still costs a permit: an attacker hammering `destroy` should meet the
    // same limiter as everyone else.
    if let Err(retry_after) = state.limiter.try_acquire() {
        return too_many(retry_after);
    }
    denied()
}

#[cfg(test)]
mod tests {
    use axum::{
        body::Body,
        http::{Request, header},
    };
    use tower::ServiceExt;

    use super::*;
    use crate::server::vault_api::{
        DENIED_MESSAGE, SEALED_MESSAGE,
        test_support::{
            DENIED_TOKEN, TOKEN, app_with, contents, guarded, guarded_contents, loaded, send,
            send_as, throttled,
        },
    };

    #[tokio::test]
    async fn a_secret_reads_back_in_vaults_shape() {
        let (status, body) = send(loaded(), "GET", "/v1/secret/data/app/db").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            body["data"]["data"]["password"],
            "duck-fixture-not-a-credential"
        );
        assert_eq!(body["data"]["metadata"]["version"], 12);
    }

    /// ADR-0015 D13: after the body has been built, the worker's decryption
    /// buffer holds nothing. The response itself carries the secret in the
    /// clear — that is what a response *is* — but nothing else does.
    #[tokio::test]
    async fn serving_a_secret_leaves_no_cleartext_behind_it() {
        let (status, body) = send(loaded(), "GET", "/v1/secret/data/app/db").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            body["data"]["data"]["password"],
            "duck-fixture-not-a-credential"
        );
        assert!(
            core_crypto::barrier::scratch_is_wiped(),
            "the worker's buffer still holds the secret after the response"
        );
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
        let app = throttled(&contents(1));

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

    // ---- Authorization (ADR-0015 D8, ADR-0013 E2/E3) ----------------------

    #[tokio::test]
    async fn a_token_reads_what_its_policy_grants() {
        let (status, body) =
            send_as(guarded(), "GET", "/v1/secret/data/app/web", Some(TOKEN)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["data"]["data"]["url"], "http://x");
    }

    #[tokio::test]
    async fn no_token_and_a_wrong_token_are_both_refused() {
        for token in [None, Some("s.nonsense"), Some("")] {
            let (status, body) = send_as(guarded(), "GET", "/v1/secret/data/app/web", token).await;
            assert_eq!(status, StatusCode::FORBIDDEN, "token {token:?} got through");
            assert_eq!(body["errors"][0], DENIED_MESSAGE);
        }
    }

    /// The SDKs are not unanimous about which header carries the token.
    #[tokio::test]
    async fn the_bearer_spelling_of_the_token_works_too() {
        let response = guarded()
            .oneshot(
                Request::builder()
                    .uri("/v1/secret/data/app/web")
                    .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    /// ADR-0013 E3, through the HTTP layer rather than the matcher.
    #[tokio::test]
    async fn an_explicit_deny_beats_the_grant_that_covers_the_same_path() {
        let app = guarded();
        let (allowed, _) = send_as(
            app.clone(),
            "GET",
            "/v1/secret/data/app/web",
            Some(DENIED_TOKEN),
        )
        .await;
        assert_eq!(allowed, StatusCode::OK, "the grant should still work");

        let (refused, body) =
            send_as(app, "GET", "/v1/secret/data/app/db", Some(DENIED_TOKEN)).await;
        assert_eq!(refused, StatusCode::FORBIDDEN);
        assert_eq!(body["errors"][0], DENIED_MESSAGE);
    }

    /// A refusal must not double as a directory listing. Same answer for a
    /// secret that exists and one that does not.
    #[tokio::test]
    async fn a_refusal_does_not_reveal_whether_the_secret_exists() {
        let app = guarded();
        let (real, real_body) =
            send_as(app.clone(), "GET", "/v1/secret/data/elsewhere/db", None).await;
        let (fake, fake_body) = send_as(app, "GET", "/v1/secret/data/elsewhere/nope", None).await;
        assert_eq!(real, StatusCode::FORBIDDEN);
        assert_eq!(real, fake);
        assert_eq!(real_body, fake_body);
    }

    /// ADR-0015 D8's quirk, end to end: listing is granted on `metadata/`,
    /// reading on `data/`, and a token with only one of them gets only one.
    #[tokio::test]
    async fn listing_needs_the_list_capability_on_the_metadata_path() {
        let app = guarded();
        for (method, uri) in [
            ("LIST", "/v1/secret/metadata/app"),
            ("GET", "/v1/secret/metadata/app?list=true"),
        ] {
            let (granted, _) = send_as(app.clone(), method, uri, Some(TOKEN)).await;
            assert_eq!(granted, StatusCode::OK, "{method} {uri}");

            // `app-minus-db` has read on `secret/data/app/*` and nothing at all
            // on `secret/metadata/`.
            let (refused, _) = send_as(app.clone(), method, uri, Some(DENIED_TOKEN)).await;
            assert_eq!(refused, StatusCode::FORBIDDEN, "{method} {uri}");
        }
    }

    /// The deployment ADR-0015 D8 says not to wait for the policy file: one
    /// app, its own file, the bucket credential as the boundary.
    #[tokio::test]
    async fn a_file_without_a_token_table_serves_without_one() {
        let (status, _) = send(loaded(), "GET", "/v1/secret/data/app/db").await;
        assert_eq!(status, StatusCode::OK);
    }

    /// A rejected token still costs a permit, or an attacker gets a free
    /// guessing loop.
    #[tokio::test]
    async fn refused_requests_are_rate_limited_too() {
        let app = throttled(&guarded_contents());

        let (first, _) = send_as(app.clone(), "GET", "/v1/secret/data/app/db", None).await;
        assert_eq!(first, StatusCode::FORBIDDEN);
        let (second, _) = send_as(app, "GET", "/v1/secret/data/app/db", None).await;
        assert_eq!(second, StatusCode::TOO_MANY_REQUESTS);
    }
}
