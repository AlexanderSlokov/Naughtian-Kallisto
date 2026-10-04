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
//!
//! This module holds what a worker's router is made of; the work itself is
//! split by responsibility, because all of it is on the read path and gets
//! read often:
//!
//! - [`request`] — every parser that touches caller-supplied text, and nothing
//!   else. It is also the fuzz surface, re-exported below as [`fuzz_api`].
//! - [`replies`] — every answer this surface can give, Vault's wording
//!   included.
//! - [`kv`] — the three handlers behind `/v1/{mount}/…`.
//! - [`observe`] — the one layer that logs and counts every request.

mod kv;
mod observe;
mod replies;
mod request;

#[cfg(test)]
pub(crate) mod test_support;

use std::sync::Arc;

use axum::{Router, middleware};
pub use replies::{DENIED_MESSAGE, SEALED_MESSAGE, TOKEN_HEADER, denied, json_response, sealed};
pub use request::presented_token;
use telemetry::{AccessLog, LogKey, Producer, WorkerMetrics};

use super::{rate_limit::RateLimiter, sys};
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
    /// The observability side, allocated once for the whole process and then
    /// sliced per worker in [`router_for_worker`].
    pub telemetry: Arc<Telemetry>,
}

/// Process-wide observability state, laid out so that a worker only ever
/// touches its own entry on the read path (ADR-0016 QĐ-3).
pub struct Telemetry {
    pub log: AccessLog,
    /// One per worker, allocated up front. A scrape lands on whichever worker
    /// `SO_REUSEPORT` gave the connection to, so every worker has to be able to
    /// read every counter — but only ever writes its own.
    pub workers: Vec<Arc<WorkerMetrics>>,
    /// The key used when the file carries no token table, and for anything
    /// refused before a file loaded at all. Process-lifetime only.
    pub fallback_log_key: LogKey,
    pub refresh_failures: Arc<std::sync::atomic::AtomicU64>,
}

impl Telemetry {
    pub fn new(queue_capacity: usize, workers: usize) -> Self {
        Self::with_log(queue_capacity, workers, true)
    }

    pub fn with_log(queue_capacity: usize, workers: usize, log_enabled: bool) -> Self {
        let workers = workers.max(1);
        Self {
            log: AccessLog::with_enabled(queue_capacity, workers, log_enabled),
            workers: (0..workers)
                .map(|_| Arc::new(WorkerMetrics::default()))
                .collect(),
            fallback_log_key: LogKey::random(),
            refresh_failures: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        }
    }
}

/// What one worker's router holds. The limiter is created in [`worker_state`],
/// which runs once per worker, so each worker counts its own traffic against
/// its own bucket (ADR-0016 QĐ-3).
#[derive(Clone)]
pub struct ApiState {
    pub resolver: Resolver,
    pub limiter: Arc<RateLimiter>,
    /// This worker's end of the access log and its own counters.
    pub producer: Arc<Producer>,
    pub metrics: Arc<WorkerMetrics>,
}

impl ApiState {
    pub fn snapshot(&self) -> Option<Arc<Snapshot>> {
        self.resolver.slot.load()
    }
}

pub fn router(resolver: Resolver) -> Router {
    router_for_worker(resolver, 0)
}

pub fn router_for_worker(resolver: Resolver, worker: usize) -> Router {
    let state = worker_state(resolver, worker);
    kv::routes()
        .merge(sys::router())
        .fallback(replies::unknown_route)
        .layer(middleware::from_fn_with_state(
            state.clone(),
            observe::observe,
        ))
        .with_state(state)
}

/// This worker's own limiter, log producer and counters — nothing here is
/// shared with another worker's read path (ADR-0016 QĐ-3).
fn worker_state(resolver: Resolver, worker: usize) -> ApiState {
    let limiter = Arc::new(RateLimiter::new(
        resolver.limits.requests_per_second,
        resolver.limits.burst,
    ));
    let producer = resolver.telemetry.log.producer(worker);
    let metrics =
        Arc::clone(&resolver.telemetry.workers[worker % resolver.telemetry.workers.len()]);
    ApiState {
        resolver,
        limiter,
        producer,
        metrics,
    }
}

/// The read path's own parsers, exposed to the fuzz targets only (ADR-0013 V3).
///
/// The reason they are worth fuzzing is in [`request`]'s module comment: they
/// run on a caller-supplied URI before any authorization decision is made.
#[cfg(feature = "fuzzing")]
pub mod fuzz_api {
    use axum::http::{Method, Uri};

    pub fn extract_mount_and_path<'a>(
        uri_path: &'a str,
        expected_action: &str,
    ) -> Option<(&'a str, &'a str)> {
        super::request::extract_mount_and_path(uri_path, expected_action)
    }

    pub fn version_param(uri: &Uri) -> Option<u64> {
        super::request::version_param(uri)
    }

    pub fn wants_list(uri: &Uri) -> bool {
        super::request::wants_list(uri)
    }

    pub fn policy_path(uri: &Uri) -> &str {
        super::request::policy_path(uri)
    }

    pub fn classify<'a>(method: &Method, uri: &'a Uri) -> (&'static str, Option<&'a str>) {
        super::observe::classify(method, uri)
    }

    pub fn presented_token(headers: &axum::http::HeaderMap) -> Option<&str> {
        super::request::presented_token(headers)
    }
}
