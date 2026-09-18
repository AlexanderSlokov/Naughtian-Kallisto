//! Atomics and interior-mutability shim.
//!
//! Under `--cfg loom` these resolve to loom's instrumented primitives so the
//! model checker can see every access; otherwise they are the std types with a
//! zero-cost wrapper. Modelled on tokio's `src/loom` layout.
//!
//! `loom::cell::UnsafeCell` only exposes the closure-based `with`/`with_mut`
//! API, so the std wrapper mirrors that shape rather than the other way around.

#[cfg(not(loom))]
pub(crate) use std::sync::atomic::{AtomicUsize, Ordering};

#[cfg(loom)]
pub(crate) use loom::{
    cell::UnsafeCell,
    sync::atomic::{AtomicUsize, Ordering},
};

#[cfg(not(loom))]
#[derive(Debug)]
pub(crate) struct UnsafeCell<T>(std::cell::UnsafeCell<T>);

#[cfg(not(loom))]
impl<T> UnsafeCell<T> {
    pub(crate) const fn new(data: T) -> Self {
        Self(std::cell::UnsafeCell::new(data))
    }

    /// Hands out a `*const T` for the duration of `f`.
    #[inline]
    pub(crate) fn with<R>(&self, f: impl FnOnce(*const T) -> R) -> R {
        f(self.0.get())
    }

    /// Hands out a `*mut T` for the duration of `f`.
    ///
    /// Takes `&self` deliberately: the caller proves exclusivity through the
    /// queue's sequence protocol, not through Rust's borrow checker. Going
    /// through `std::cell::UnsafeCell::get` is what makes the resulting pointer
    /// carry write permission — casting a `*const T` obtained from a shared
    /// reference does not, and Miri rejects it under Stacked Borrows.
    #[inline]
    pub(crate) fn with_mut<R>(&self, f: impl FnOnce(*mut T) -> R) -> R {
        f(self.0.get())
    }
}

/// Retry hint for the CAS loops.
///
/// In production this is the `PAUSE` instruction, which is what you want in a
/// contended CAS retry loop anyway. Under loom it is a yield point, which is
/// what stops the model checker from treating the retry loop as unbounded
/// spinning and blowing its branch budget.
#[cfg(loom)]
#[inline]
pub(crate) fn spin_hint() {
    loom::thread::yield_now();
}

#[cfg(not(loom))]
#[inline]
pub(crate) fn spin_hint() {
    std::hint::spin_loop();
}
