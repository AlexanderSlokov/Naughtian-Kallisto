//! One access-log line and one counter per request, for every route.

use axum::{
    extract::{Request, State},
    http::{Method, Uri},
    middleware::Next,
    response::Response,
};
use telemetry::{Id, Outcome, Record};

use super::{
    ApiState,
    request::{extract_mount_and_path, is_list_method, presented_token, wants_list},
};
use crate::resolver::snapshot::Snapshot;

/// What a log line needs from a request, all of it fixed-size, taken before the
/// request is handed on.
struct Observed {
    method: Method,
    action: &'static str,
    path: Id,
    token: Id,
    enforced: bool,
}

impl Observed {
    /// Everything identifying is turned into a fixed-size [`Id`] *before* the
    /// request is handed on. Doing it afterwards would mean keeping the token
    /// as an owned `String` across the await — one heap allocation per
    /// request, on the path this log exists not to burden. It also means
    /// one `ArcSwap` load instead of two, since the handler loads the
    /// snapshot for itself anyway.
    fn of(state: &ApiState, request: &Request) -> Self {
        let snapshot = state.snapshot();
        let (action, secret_path) = classify(request.method(), request.uri());
        let (path, token) = identify(state, request, snapshot.as_deref(), secret_path);
        Self {
            // Cheap for every standard verb; `LIST` is an extension method and
            // is the one spelling that allocates here.
            method: request.method().clone(),
            action,
            path,
            token,
            enforced: snapshot.is_some_and(|s| s.enforces()),
        }
    }
}

/// The two identifiers a line carries, derived while the request is still here.
fn identify(
    state: &ApiState,
    request: &Request,
    snapshot: Option<&Snapshot>,
    secret_path: Option<&str>,
) -> (Id, Id) {
    let fallback = &state.resolver.telemetry.fallback_log_key;
    match (snapshot, secret_path) {
        (Some(snapshot), Some(path)) => (
            snapshot.path_id(path, fallback),
            snapshot.token_id(presented_token(request.headers())),
        ),
        (None, Some(path)) => (fallback.id(path), Id::NONE),
        (_, None) => (Id::NONE, Id::NONE),
    }
}

/// A layer rather than a call in each handler, deliberately. An access log's
/// entire value is that *every* request appears in it; spread across a dozen
/// branches, the one that gets forgotten is invisible — the tests still pass,
/// the log still looks healthy, and the missing requests are the interesting
/// ones. Here there is one place, and it cannot be bypassed by adding a route.
///
/// The cost is one extra `ArcSwap` load per request (the handler loads the
/// snapshot too) and a boxed future for the layer. Both are measured against
/// the M5 numbers rather than assumed.
pub(super) async fn observe(
    State(state): State<ApiState>,
    request: Request,
    next: Next,
) -> Response {
    let observed = Observed::of(&state, &request);
    let response = next.run(request).await;

    let status = response.status();
    state.metrics.observe(Outcome::of(status.as_u16()));
    state.producer.record(&Record {
        method: observed.method.as_str(),
        action: observed.action,
        path: observed.path,
        token: observed.token,
        status: status.as_u16(),
        enforced: observed.enforced,
    });

    response
}

/// What kind of request this was, and which secret path it named.
///
/// The action is drawn from a fixed set of words, never from the URI, so no
/// caller-supplied text can reach a log line unhashed. The path comes back as a
/// borrow for the caller to identify; it is never logged as itself.
pub(super) fn classify<'a>(method: &Method, uri: &'a Uri) -> (&'static str, Option<&'a str>) {
    let listing = is_list_method(method) || wants_list(uri);
    if let Some((action, path)) = classify_read(uri, listing) {
        return (action, Some(path));
    }
    if let Some(path) = classify_write(uri) {
        return ("write", Some(path));
    }
    if uri.path().starts_with("/v1/sys/") || uri.path().starts_with("/v1/auth/") {
        return ("sys", None);
    }
    ("-", None)
}

fn classify_read(uri: &Uri, listing: bool) -> Option<(&'static str, &str)> {
    for action in ["data", "metadata"] {
        if let Some((_, path)) = extract_mount_and_path(uri.path(), action) {
            let label = if action == "metadata" && listing {
                "list"
            } else {
                action
            };
            return Some((label, path));
        }
    }
    None
}

fn classify_write(uri: &Uri) -> Option<&str> {
    ["subkeys", "delete", "undelete", "destroy"]
        .into_iter()
        .find_map(|action| extract_mount_and_path(uri.path(), action))
        .map(|(_, path)| path)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::{
        body::Body,
        http::{Request, StatusCode, header},
    };
    use tower::ServiceExt;

    use crate::server::vault_api::{
        Telemetry,
        test_support::{TOKEN, observed, observed_app_with, send_as},
    };

    fn drain(telemetry: &Telemetry) -> Vec<String> {
        let sink = Arc::new(std::sync::Mutex::new(Vec::new()));
        let handle = telemetry.log.spawn_writer(TestSink(Arc::clone(&sink)));
        std::thread::sleep(std::time::Duration::from_millis(40));
        telemetry.log.stop();
        handle.join().unwrap();
        String::from_utf8(sink.lock().unwrap().clone())
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    struct TestSink(Arc<std::sync::Mutex<Vec<u8>>>);
    impl std::io::Write for TestSink {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    async fn scrape(telemetry: &Arc<Telemetry>) -> (StatusCode, String) {
        let response = observed(Arc::clone(telemetry))
            .oneshot(
                Request::builder()
                    .uri("/v1/sys/metrics")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let content_type = response
            .headers()
            .get(header::CONTENT_TYPE)
            .map(|v| v.to_str().unwrap().to_owned());
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(
            content_type.as_deref(),
            Some("text/plain; version=0.0.4; charset=utf-8")
        );
        (status, String::from_utf8(bytes.to_vec()).unwrap())
    }

    async fn read_db(telemetry: &Arc<Telemetry>, token: Option<&str>) -> StatusCode {
        let (status, _) = send_as(
            observed(Arc::clone(telemetry)),
            "GET",
            "/v1/secret/data/app/db",
            token,
        )
        .await;
        status
    }

    /// The property that makes an access log worth having: every request is in
    /// it. A layer rather than a call per handler is what guarantees this, and
    /// this is the assertion that would catch a route added without one.
    #[tokio::test]
    async fn every_route_produces_exactly_one_line() {
        let telemetry = Arc::new(Telemetry::new(256, 1));

        for (method, uri) in [
            ("GET", "/v1/secret/data/app/db"),
            ("GET", "/v1/secret/data/nope"),
            ("PUT", "/v1/secret/data/app/db"),
            ("LIST", "/v1/secret/metadata/app"),
            ("GET", "/v1/secret/metadata/app/db"),
            ("DELETE", "/v1/secret/destroy/app/db"),
            ("GET", "/v1/sys/health"),
            ("GET", "/v1/nonsense"),
        ] {
            let _ = send_as(observed(Arc::clone(&telemetry)), method, uri, Some(TOKEN)).await;
        }

        let lines = drain(&telemetry);
        assert_eq!(lines.len(), 8, "{lines:#?}");
        for line in &lines {
            assert!(line.contains("status="), "{line}");
            assert!(line.contains("path="), "{line}");
        }
    }

    /// The identifier written for a served path is the *precomputed* one, and
    /// it matches what the same key produces on demand. If these ever diverged
    /// the log would be useless while every other test stayed green.
    #[tokio::test]
    async fn the_logged_path_identifier_matches_the_key_the_file_carries() {
        let telemetry = Arc::new(Telemetry::new(64, 1));
        assert_eq!(read_db(&telemetry, Some(TOKEN)).await, StatusCode::OK);

        // Derived here from the token key alone, *not* by asking the snapshot —
        // a check that routes through `path_id` would move with the thing it
        // is checking, and pass no matter what either side hashed. (The M4 E2
        // test made exactly that mistake and survived a broken implementation.)
        let expected =
            telemetry::LogKey::derived_from(&crate::server::vault_api::test_support::token_key())
                .id("app/db");

        let lines = drain(&telemetry);
        assert!(
            lines[0].contains(&format!("path={expected}")),
            "expected {expected} in {}",
            lines[0]
        );
        assert!(lines[0].contains("action=data"));
        assert!(lines[0].contains("status=200"));
    }

    /// QĐ-7's payoff: the token identifier in a log line is byte-for-byte the
    /// key of that token's row in the sealed file, so an operator holding the
    /// file reads the line without reversing anything.
    #[tokio::test]
    async fn the_logged_token_identifier_is_the_files_own_token_hash() {
        let telemetry = Arc::new(Telemetry::new(64, 1));
        let _ = read_db(&telemetry, Some(TOKEN)).await;

        let in_file = crate::server::vault_api::test_support::token_key().hash_hex(TOKEN);
        let lines = drain(&telemetry);
        let logged = lines[0]
            .split(" token=")
            .nth(1)
            .and_then(|rest| rest.split(' ').next())
            .unwrap();
        assert!(
            in_file.starts_with(logged),
            "log wrote {logged}, file holds {in_file}"
        );
    }

    /// Nothing a caller controls reaches a line as text.
    #[tokio::test]
    async fn a_caller_chosen_path_is_hashed_rather_than_written() {
        let telemetry = Arc::new(Telemetry::new(64, 1));
        let _ = send_as(
            observed(Arc::clone(&telemetry)),
            "GET",
            "/v1/secret/data/looking-for/../../etc/passwd",
            Some(TOKEN),
        )
        .await;

        let lines = drain(&telemetry);
        assert!(!lines[0].contains("passwd"), "{}", lines[0]);
        assert!(!lines[0].contains("looking-for"), "{}", lines[0]);
    }

    /// D15: a full queue costs log lines, never a served request. This is the
    /// difference between this and an audit log, asserted rather than asserted
    /// about.
    #[tokio::test]
    async fn a_full_log_queue_never_stops_a_read() {
        // Two slots and no writer running, so the queue fills immediately.
        let telemetry = Arc::new(Telemetry::new(2, 1));

        for _ in 0..40 {
            assert_eq!(read_db(&telemetry, Some(TOKEN)).await, StatusCode::OK);
        }

        assert!(
            telemetry.log.dropped() > 0,
            "a queue of two absorbed forty lines"
        );
    }

    #[tokio::test]
    async fn the_metrics_endpoint_counts_what_was_served() {
        let telemetry = Arc::new(Telemetry::new(256, 1));
        let _ = read_db(&telemetry, Some(TOKEN)).await;
        let _ = read_db(&telemetry, Some("s.wrong")).await;

        let (status, text) = scrape(&telemetry).await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            text.contains("kallisto_access_log_dropped_total 0"),
            "{text}"
        );
        assert!(
            text.contains("kallisto_requests_total{outcome=\"ok\"} 1"),
            "{text}"
        );
        assert!(
            text.contains("kallisto_requests_total{outcome=\"denied\"} 1"),
            "{text}"
        );
        assert!(text.contains("kallisto_sealed 0"), "{text}");
        // The scrape itself is counted too, so the numbers add up for anyone
        // reading them rather than quietly excluding one route.
        assert!(text.contains("kallisto_authorization_enforced 1"), "{text}");
    }

    /// QĐ-8, end to end: the number an alert fires on has to reach a scrape.
    #[tokio::test]
    async fn dropped_lines_reach_the_metrics_endpoint() {
        let telemetry = Arc::new(Telemetry::new(2, 1));
        for _ in 0..40 {
            let _ = read_db(&telemetry, Some(TOKEN)).await;
        }

        let (_, text) = scrape(&telemetry).await;
        let dropped: u64 = text
            .lines()
            .find_map(|l| l.strip_prefix("kallisto_access_log_dropped_total "))
            .unwrap()
            .parse()
            .unwrap();
        assert!(dropped > 0, "the drop count never reached the scrape");
    }

    /// A sealed process still answers a scrape — that is when it matters most.
    #[tokio::test]
    async fn metrics_answer_while_sealed() {
        let telemetry = Arc::new(Telemetry::new(64, 1));
        let response = observed_app_with(None, Arc::clone(&telemetry))
            .oneshot(
                Request::builder()
                    .uri("/v1/sys/metrics")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let text = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(text.contains("kallisto_sealed 1"), "{text}");
    }
}
