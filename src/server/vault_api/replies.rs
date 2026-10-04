//! Every answer this surface can give.
//!
//! Several of these strings are Vault's own. Clients match on them, so they are
//! compatibility surface rather than messages we are free to improve.

use axum::{
    http::{StatusCode, Uri, header},
    response::{IntoResponse, Response},
};

use crate::server::responses;

pub const SEALED_MESSAGE: &str = "Vault is sealed";
pub const DENIED_MESSAGE: &str = "permission denied";

/// Vault's own header. `Authorization: Bearer` is accepted too, because the
/// SDKs are not unanimous and an app that sets the wrong one of the two gets a
/// 403 that looks like a policy problem.
pub const TOKEN_HEADER: &str = "x-vault-token";

pub fn json_response(status: StatusCode, body: String) -> Response {
    (status, [(header::CONTENT_TYPE, "application/json")], body).into_response()
}

pub fn sealed() -> Response {
    json_response(
        StatusCode::SERVICE_UNAVAILABLE,
        responses::errors(SEALED_MESSAGE),
    )
}

pub fn denied() -> Response {
    json_response(StatusCode::FORBIDDEN, responses::errors(DENIED_MESSAGE))
}

/// Vault answers a missing KV-v2 secret with a 404 and an empty error list.
pub(super) fn absent() -> Response {
    json_response(StatusCode::NOT_FOUND, r#"{"errors":[]}"#.to_string())
}

pub(super) fn too_many(retry_after_secs: u64) -> Response {
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

pub(super) fn route_not_found(path: &str) -> Response {
    let route = path.strip_prefix("/v1/").unwrap_or(path);
    json_response(
        StatusCode::NOT_FOUND,
        responses::errors(&format!(
            "no handler for route \"{route}\". route entry not found."
        )),
    )
}

/// The router's fallback: anything no route claimed.
pub(super) async fn unknown_route(uri: Uri) -> Response {
    route_not_found(uri.path())
}
