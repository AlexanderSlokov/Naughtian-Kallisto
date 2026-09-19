//! A token bucket per worker.
//!
//! ADR-0015 D14 says Kallisto answers an overload the way Vault does — 429 with
//! `Retry-After` — rather than the way S3 does. This is the bucket behind that
//! number.
//!
//! One bucket per worker, not one per process. ADR-0016 QĐ-3 kept
//! thread-per-core specifically so the serving path never shares a cache line;
//! a process-wide limiter would put a contended atomic in front of every read
//! and undo that. The configured rate is therefore per worker, and the
//! configuration field says so in its name.

use std::{
    sync::atomic::{AtomicU64, Ordering},
    time::Instant,
};

/// Fixed point: one permit is a million units, so a rate of one request per
/// second still refills smoothly at millisecond resolution.
const ONE_PERMIT: u64 = 1_000_000;

pub struct RateLimiter {
    capacity: u64,
    per_millisecond: u64,
    permits: AtomicU64,
    last_refill_ms: AtomicU64,
    origin: Instant,
}

impl RateLimiter {
    /// `per_second` permits, `burst` of them available at once.
    pub fn new(per_second: u64, burst: u64) -> Self {
        let capacity = burst.max(1).saturating_mul(ONE_PERMIT);
        Self {
            capacity,
            per_millisecond: per_second.max(1).saturating_mul(ONE_PERMIT) / 1000,
            permits: AtomicU64::new(capacity),
            last_refill_ms: AtomicU64::new(0),
            origin: Instant::now(),
        }
    }

    /// `Ok` to serve; `Err(seconds)` to answer 429 with that `Retry-After`.
    ///
    /// The atomics are `Relaxed` on purpose: a bucket belongs to exactly one
    /// worker thread, so this is uncontended by construction. The atomics are
    /// here to satisfy `Sync` for the handler state, not to synchronise
    /// anything. The worst a torn interleaving could do is let one extra
    /// request through, which is not a property anyone is relying on.
    pub fn try_acquire(&self) -> Result<(), u64> {
        self.try_acquire_at(self.origin.elapsed().as_millis() as u64)
    }

    fn try_acquire_at(&self, now_ms: u64) -> Result<(), u64> {
        let last = self.last_refill_ms.load(Ordering::Relaxed);
        if now_ms > last {
            self.last_refill_ms.store(now_ms, Ordering::Relaxed);
            let refilled = self
                .permits
                .load(Ordering::Relaxed)
                .saturating_add((now_ms - last).saturating_mul(self.per_millisecond))
                .min(self.capacity);
            self.permits.store(refilled, Ordering::Relaxed);
        }

        let available = self.permits.load(Ordering::Relaxed);
        if available >= ONE_PERMIT {
            self.permits
                .store(available - ONE_PERMIT, Ordering::Relaxed);
            return Ok(());
        }

        // How long until one permit exists again, rounded up, and never zero —
        // a `Retry-After: 0` is an invitation to hot-loop.
        let missing = ONE_PERMIT - available;
        let wait_ms = missing.div_ceil(self.per_millisecond.max(1));
        Err(wait_ms.div_ceil(1000).max(1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_full_bucket_serves_its_burst_and_then_refuses() {
        let limiter = RateLimiter::new(10, 3);
        for i in 0..3 {
            assert!(limiter.try_acquire_at(0).is_ok(), "refused request {i}");
        }
        assert!(limiter.try_acquire_at(0).is_err(), "burst was not a limit");
    }

    #[test]
    fn it_refills_over_time() {
        let limiter = RateLimiter::new(1000, 2);
        limiter.try_acquire_at(0).unwrap();
        limiter.try_acquire_at(0).unwrap();
        limiter.try_acquire_at(0).unwrap_err();

        // 1000/s is one per millisecond.
        limiter.try_acquire_at(1).unwrap();
        limiter.try_acquire_at(1).unwrap_err();
    }

    #[test]
    fn it_does_not_refill_beyond_the_burst() {
        let limiter = RateLimiter::new(1000, 2);
        let _ = limiter.try_acquire_at(0);
        let _ = limiter.try_acquire_at(0);
        // An hour of idleness is still a burst of two.
        for _ in 0..2 {
            limiter.try_acquire_at(3_600_000).unwrap();
        }
        limiter.try_acquire_at(3_600_000).unwrap_err();
    }

    /// A client that honours `Retry-After: 0` will come back immediately and
    /// be refused again, forever.
    #[test]
    fn retry_after_is_never_zero() {
        let limiter = RateLimiter::new(1, 1);
        limiter.try_acquire_at(0).unwrap();
        assert_eq!(limiter.try_acquire_at(0).unwrap_err(), 1);
    }

    /// A slow rate must not divide by zero when converting to per-millisecond.
    #[test]
    fn a_rate_below_one_per_millisecond_still_works() {
        let limiter = RateLimiter::new(1, 1);
        limiter.try_acquire_at(0).unwrap();
        limiter.try_acquire_at(500).unwrap_err();
        limiter.try_acquire_at(1000).unwrap();
    }
}
