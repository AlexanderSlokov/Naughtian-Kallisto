//! Everything that reads caller-supplied text — the URI, the query string, the
//! headers. Nothing here decides anything; it only parses.
//!
//! One module because this is exactly the surface the fuzz targets cover
//! (ADR-0013 V3, re-exported as `vault_api::fuzz_api`). Every one of these runs
//! before any authorization decision is made, and three of them index into a
//! query string by byte offset after a `find` — which is where a multi-byte
//! UTF-8 boundary panics. A panic in a worker takes that worker's connection
//! down, so this is a liveness surface as well as a correctness one.

use axum::http::{HeaderMap, Method, Uri, header};

use super::replies::TOKEN_HEADER;

/// The token an app presented, if any.
///
/// Note what this does *not* do: distinguish a missing token from a wrong one.
/// Real Vault answers 400 `missing client token` for the first and 403 for the
/// second. Both are 403 here, deliberately — the difference is an oracle, and
/// no client does anything useful with it that a 403 does not also tell them.
pub fn presented_token(headers: &HeaderMap) -> Option<&str> {
    if let Some(value) = headers.get(TOKEN_HEADER)
        && let Ok(token) = value.to_str()
        && !token.is_empty()
    {
        return Some(token);
    }
    headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .filter(|token| !token.is_empty())
}

/// The path a policy is written against: the request path with `/v1/` removed,
/// mount and `data/`-or-`metadata/` segment still in place (ADR-0015 D8).
///
/// A borrow rather than a `format!`, because this runs on every read.
#[inline]
pub(super) fn policy_path(uri: &Uri) -> &str {
    let path = uri.path();
    path.strip_prefix("/v1/").unwrap_or(path)
}

/// `/v1/{mount}/{action}/{path}` split into its three parts.
///
/// Carried over unchanged from the engine-era handler, tests included: it was
/// correct, it is on the hot path, and rewriting working string slicing is how
/// a migration acquires bugs it did not have before.
pub(super) fn extract_mount_and_path<'a>(
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
pub(super) fn version_param(uri: &Uri) -> Option<u64> {
    let query = uri.query()?;
    let start = query.find("version=")? + "version=".len();
    let rest = &query[start..];
    let end = rest.find('&').unwrap_or(rest.len());
    rest[..end].parse::<u64>().ok()
}

#[inline]
pub(super) fn wants_list(uri: &Uri) -> bool {
    uri.query().is_some_and(|q| q.contains("list=true"))
}

/// Vault accepts the made-up `LIST` verb as well as `GET ?list=true`, and the
/// SDKs are split on which they send: the Go client sends `LIST`, several
/// others send the query parameter. Supporting only one of them passes every
/// unit test and fails `vault kv list`.
#[inline]
pub(super) fn is_list_method(method: &Method) -> bool {
    method.as_str().eq_ignore_ascii_case("LIST")
}

#[inline]
pub(super) fn is_read_method(method: &Method) -> bool {
    method == Method::GET || method == Method::HEAD
}

#[cfg(test)]
mod tests {
    use super::*;

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
