//! Counters, and the Prometheus text they render to.
//!
//! The one that matters is `kallisto_access_log_dropped_total` (ADR-0015 D15,
//! QĐ-8). A non-zero value means the daemon is shedding its own bookkeeping to
//! keep serving secrets — the right trade, and exactly the thing an operator
//! should be woken for, because something is putting load on a process that
//! comfortably handles tens of thousands of reads a second. A field buried in a
//! JSON health body cannot raise an alert; a scraped counter can.
//!
//! Counters are **per worker** and summed only when somebody scrapes. A single
//! shared counter would be a contended cache line on the read path, which is
//! what ADR-0016 QĐ-3 rearranged the whole server to avoid.

use std::sync::atomic::{AtomicU64, Ordering};

/// The status classes worth separating. A label per distinct status code would
/// be unbounded cardinality; these are the five answers this server gives.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Ok,
    Denied,
    NotFound,
    RateLimited,
    Sealed,
    Error,
}

impl Outcome {
    pub fn of(status: u16) -> Self {
        match status {
            200..=299 => Self::Ok,
            403 => Self::Denied,
            404 => Self::NotFound,
            429 => Self::RateLimited,
            503 => Self::Sealed,
            _ => Self::Error,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Denied => "denied",
            Self::NotFound => "not_found",
            Self::RateLimited => "rate_limited",
            Self::Sealed => "sealed",
            Self::Error => "error",
        }
    }

    const ALL: [Self; 6] = [
        Self::Ok,
        Self::Denied,
        Self::NotFound,
        Self::RateLimited,
        Self::Sealed,
        Self::Error,
    ];
}

/// One worker's counters. Nothing here is shared with another worker, so the
/// orderings are `Relaxed` for the same reason `RateLimiter`'s are: the atomics
/// exist to satisfy `Sync`, not to synchronise anything.
#[derive(Default)]
pub struct WorkerMetrics {
    ok: AtomicU64,
    denied: AtomicU64,
    not_found: AtomicU64,
    rate_limited: AtomicU64,
    sealed: AtomicU64,
    error: AtomicU64,
}

impl WorkerMetrics {
    pub fn observe(&self, outcome: Outcome) {
        self.counter(outcome).fetch_add(1, Ordering::Relaxed);
    }

    fn counter(&self, outcome: Outcome) -> &AtomicU64 {
        match outcome {
            Outcome::Ok => &self.ok,
            Outcome::Denied => &self.denied,
            Outcome::NotFound => &self.not_found,
            Outcome::RateLimited => &self.rate_limited,
            Outcome::Sealed => &self.sealed,
            Outcome::Error => &self.error,
        }
    }

    pub fn get(&self, outcome: Outcome) -> u64 {
        self.counter(outcome).load(Ordering::Relaxed)
    }
}

/// What the scrape handler needs that the counters do not carry.
pub struct Status {
    pub sealed: bool,
    pub file_version: Option<u64>,
    pub secrets: usize,
    pub tokens: usize,
    pub enforces: bool,
    pub log_dropped: u64,
    pub refresh_failures: u64,
}

/// Prometheus text exposition (version 0.0.4).
///
/// Built with `push_str`, like `responses.rs` builds Vault's JSON, and for the
/// same reason: a handful of fixed lines does not need a framework.
pub fn render(workers: &[impl AsRef<WorkerMetrics>], status: &Status) -> String {
    let mut out = String::with_capacity(1024);

    out.push_str("# HELP kallisto_access_log_dropped_total Access log lines discarded because the queue was full.\n");
    out.push_str("# TYPE kallisto_access_log_dropped_total counter\n");
    gauge(
        &mut out,
        "kallisto_access_log_dropped_total",
        status.log_dropped,
    );

    out.push_str("# HELP kallisto_requests_total Requests answered, by outcome class.\n");
    out.push_str("# TYPE kallisto_requests_total counter\n");
    for outcome in Outcome::ALL {
        let total: u64 = workers.iter().map(|w| w.as_ref().get(outcome)).sum();
        out.push_str("kallisto_requests_total{outcome=\"");
        out.push_str(outcome.label());
        out.push_str("\"} ");
        push_u64(&mut out, total);
        out.push('\n');
    }

    out.push_str("# HELP kallisto_sealed Whether no file has loaded, so every read answers 503.\n");
    out.push_str("# TYPE kallisto_sealed gauge\n");
    gauge(&mut out, "kallisto_sealed", u64::from(status.sealed));

    out.push_str("# HELP kallisto_file_version Content version of the sealed file being served.\n");
    out.push_str("# TYPE kallisto_file_version gauge\n");
    gauge(
        &mut out,
        "kallisto_file_version",
        status.file_version.unwrap_or(0),
    );

    out.push_str("# HELP kallisto_secrets Secrets in the file being served.\n");
    out.push_str("# TYPE kallisto_secrets gauge\n");
    gauge(&mut out, "kallisto_secrets", status.secrets as u64);

    out.push_str("# HELP kallisto_tokens Tokens in the file being served.\n");
    out.push_str("# TYPE kallisto_tokens gauge\n");
    gauge(&mut out, "kallisto_tokens", status.tokens as u64);

    out.push_str(
        "# HELP kallisto_authorization_enforced Whether the file carries a token table at all.\n",
    );
    out.push_str("# TYPE kallisto_authorization_enforced gauge\n");
    gauge(
        &mut out,
        "kallisto_authorization_enforced",
        u64::from(status.enforces),
    );

    out.push_str(
        "# HELP kallisto_refresh_failures_total Polls that did not yield a servable file.\n",
    );
    out.push_str("# TYPE kallisto_refresh_failures_total counter\n");
    gauge(
        &mut out,
        "kallisto_refresh_failures_total",
        status.refresh_failures,
    );

    out
}

fn gauge(out: &mut String, name: &str, value: u64) {
    out.push_str(name);
    out.push(' ');
    push_u64(out, value);
    out.push('\n');
}

fn push_u64(out: &mut String, value: u64) {
    let mut buffer = [0u8; 20];
    let mut at = buffer.len();
    let mut value = value;
    loop {
        at -= 1;
        buffer[at] = b'0' + (value % 10) as u8;
        value /= 10;
        if value == 0 {
            break;
        }
    }
    out.push_str(std::str::from_utf8(&buffer[at..]).unwrap_or("0"));
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    fn status() -> Status {
        Status {
            sealed: false,
            file_version: Some(42),
            secrets: 7,
            tokens: 2,
            enforces: true,
            log_dropped: 0,
            refresh_failures: 0,
        }
    }

    /// Parses the subset of the exposition format this emits, so the assertions
    /// below test the output the way a scraper would read it.
    fn scrape(text: &str) -> Vec<(String, u64)> {
        text.lines()
            .filter(|line| !line.starts_with('#'))
            .filter_map(|line| {
                let (name, value) = line.rsplit_once(' ')?;
                Some((name.to_string(), value.parse().ok()?))
            })
            .collect()
    }

    fn value(text: &str, name: &str) -> u64 {
        scrape(text)
            .into_iter()
            .find(|(n, _)| n == name)
            .unwrap_or_else(|| panic!("{name} missing from:\n{text}"))
            .1
    }

    #[test]
    fn counters_from_every_worker_are_summed() {
        let workers: Vec<Arc<WorkerMetrics>> =
            (0..3).map(|_| Arc::new(WorkerMetrics::default())).collect();
        workers[0].observe(Outcome::Ok);
        workers[1].observe(Outcome::Ok);
        workers[1].observe(Outcome::Ok);
        workers[2].observe(Outcome::Denied);

        let text = render(&workers, &status());
        assert_eq!(value(&text, "kallisto_requests_total{outcome=\"ok\"}"), 3);
        assert_eq!(
            value(&text, "kallisto_requests_total{outcome=\"denied\"}"),
            1
        );
        assert_eq!(
            value(&text, "kallisto_requests_total{outcome=\"not_found\"}"),
            0
        );
    }

    /// QĐ-8: this is the number an operator gets paged on, so it has to be
    /// present and scrapeable even at zero — a metric that only appears once it
    /// is broken cannot have an alert written against it.
    #[test]
    fn the_dropped_counter_is_always_present() {
        let workers = [Arc::new(WorkerMetrics::default())];
        assert_eq!(
            value(
                &render(&workers, &status()),
                "kallisto_access_log_dropped_total"
            ),
            0
        );

        let flooded = Status {
            log_dropped: 9_001,
            ..status()
        };
        assert_eq!(
            value(
                &render(&workers, &flooded),
                "kallisto_access_log_dropped_total"
            ),
            9_001
        );
    }

    #[test]
    fn the_served_file_is_reported_as_gauges() {
        let workers = [Arc::new(WorkerMetrics::default())];
        let text = render(&workers, &status());
        assert_eq!(value(&text, "kallisto_file_version"), 42);
        assert_eq!(value(&text, "kallisto_sealed"), 0);
        assert_eq!(value(&text, "kallisto_secrets"), 7);
        assert_eq!(value(&text, "kallisto_authorization_enforced"), 1);
    }

    #[test]
    fn a_sealed_process_says_so_and_reports_no_version() {
        let workers = [Arc::new(WorkerMetrics::default())];
        let text = render(
            &workers,
            &Status {
                sealed: true,
                file_version: None,
                secrets: 0,
                tokens: 0,
                enforces: false,
                ..status()
            },
        );
        assert_eq!(value(&text, "kallisto_sealed"), 1);
        assert_eq!(value(&text, "kallisto_file_version"), 0);
    }

    /// Every metric needs its HELP and TYPE or a scraper treats it as untyped.
    #[test]
    fn every_metric_is_documented_and_typed() {
        let workers = [Arc::new(WorkerMetrics::default())];
        let text = render(&workers, &status());
        for (name, _) in scrape(&text) {
            let bare = name.split('{').next().unwrap();
            assert!(
                text.contains(&format!("# HELP {bare} ")),
                "{bare} has no HELP"
            );
            assert!(
                text.contains(&format!("# TYPE {bare} ")),
                "{bare} has no TYPE"
            );
        }
    }

    #[test]
    fn status_codes_map_onto_the_closed_set_of_labels() {
        assert!(matches!(Outcome::of(200), Outcome::Ok));
        assert!(matches!(Outcome::of(403), Outcome::Denied));
        assert!(matches!(Outcome::of(404), Outcome::NotFound));
        assert!(matches!(Outcome::of(429), Outcome::RateLimited));
        assert!(matches!(Outcome::of(503), Outcome::Sealed));
        assert!(matches!(Outcome::of(500), Outcome::Error));
    }
}
