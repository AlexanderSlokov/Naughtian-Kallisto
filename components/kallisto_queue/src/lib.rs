//! Dmitry Vyukov's bounded MPMC lock-free queue.
//!
//! Extracted from `naughtian-kallisto` so that `loom` can model-check it
//! (ADR-0013 invariants B1/B2). `make loom` compiles with
//! `RUSTFLAGS="--cfg loom"`, which tokio and hyper-util also react to — tokio
//! drops `tokio::net` under that cfg and hyper-util then fails to build.
//! Keeping the queue in a crate whose dependency graph is `loom` and nothing
//! else is what makes the model checker usable at all.

#![cfg_attr(docsrs, feature(doc_cfg))]

mod sync;

use std::mem::MaybeUninit;

use crate::sync::{AtomicUsize, Ordering, UnsafeCell, spin_hint};

#[cfg(all(loom, not(feature = "loom")))]
compile_error!(
    "RUSTFLAGS=\"--cfg loom\" requires --features loom (see the `loom` target in the Makefile)"
);

#[derive(Debug, PartialEq, Eq)]
pub enum QueueError {
    Full,
    Empty,
}

#[repr(C, align(64))]
struct Node<T> {
    sequence: AtomicUsize,
    data: UnsafeCell<MaybeUninit<T>>,
}

#[repr(align(64))]
struct CachePadded<T>(T);

/// Bounded MPMC queue. Capacity must be a power of two.
///
/// Provides ultra-low latency lock-free message passing: no OS context
/// switches, mutexes, or cond_vars on the hot path.
///
/// # Example
/// ```
/// use kallisto_queue::LockFreeQueue;
///
/// let q = LockFreeQueue::new(2);
/// q.enqueue("a").unwrap();
/// assert_eq!(q.dequeue().unwrap(), "a");
/// assert!(q.dequeue().is_err());
/// ```
pub struct LockFreeQueue<T> {
    buffer: Box<[Node<T>]>,
    enqueue_pos: CachePadded<AtomicUsize>,
    dequeue_pos: CachePadded<AtomicUsize>,
}

// SAFETY: All shared state (`enqueue_pos`, `dequeue_pos`, `Node::sequence`) is
// accessed exclusively through atomic operations. `Node::data` sits behind an
// `UnsafeCell` and is only written by the thread that won the CAS on
// `enqueue_pos`, and only read by the thread that won the CAS on `dequeue_pos`,
// so at most one thread touches a slot's data at a time. The `T: Send` bound
// guarantees the payload itself is safe to transfer across threads.
unsafe impl<T: Send> Send for LockFreeQueue<T> {}
unsafe impl<T: Send> Sync for LockFreeQueue<T> {}

impl<T> LockFreeQueue<T> {
    /// # Panics
    /// If `capacity` is not a power of two, or is less than 2.
    ///
    /// Capacity 1 is rejected even though it is a power of two: the algorithm
    /// marks a filled slot with `sequence = pos + 1` and a drained slot with
    /// `sequence = pos + capacity`, which are the same value when
    /// `capacity == 1`. A capacity-1 queue therefore accepts a second enqueue
    /// over the unconsumed first one (losing and leaking it) and then spins
    /// forever in `dequeue`. Found by the loom model in `loom_tests`.
    pub fn new(capacity: usize) -> Self {
        assert!(
            capacity.is_power_of_two() && capacity >= 2,
            "Capacity must be a power of 2 and at least 2, got {capacity}"
        );
        let mut buffer = Vec::with_capacity(capacity);
        for i in 0..capacity {
            buffer.push(Node {
                sequence: AtomicUsize::new(i),
                data: UnsafeCell::new(MaybeUninit::uninit()),
            });
        }

        Self {
            buffer: buffer.into_boxed_slice(),
            enqueue_pos: CachePadded(AtomicUsize::new(0)),
            dequeue_pos: CachePadded(AtomicUsize::new(0)),
        }
    }

    pub fn enqueue(&self, data: T) -> Result<(), QueueError> {
        let capacity = self.buffer.len();
        let mask = capacity - 1;
        let mut pos = self.enqueue_pos.0.load(Ordering::Relaxed);

        loop {
            let cell = &self.buffer[pos & mask];
            let seq = cell.sequence.load(Ordering::Acquire);

            let dif = (seq as isize) - (pos as isize);
            if dif == 0 {
                // On failure the CAS reports who actually holds the slot; adopt
                // it. Vyukov's C++ original gets this for free because
                // `compare_exchange_weak` updates `pos` by reference. Discarding
                // it and retrying with a stale `pos` spins against the winner's
                // not-yet-published `sequence` store — a livelock the loom model
                // reports as unbounded branching.
                match self.enqueue_pos.0.compare_exchange_weak(
                    pos,
                    pos + 1,
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                ) {
                    Ok(_) => {
                        cell.data.with_mut(|slot| {
                            // SAFETY: Winning the CAS on `enqueue_pos` claims slot
                            // `pos & mask` exclusively, and `sequence == pos` proves
                            // the previous occupant was already moved out by
                            // `dequeue` (or the slot was never initialised), so this
                            // write does not overwrite a live value and cannot
                            // double-drop. The pointer comes from `UnsafeCell`, so it
                            // carries write provenance. The `Release` store below
                            // publishes the write to whoever reads `sequence`.
                            unsafe { (*slot).write(data) };
                        });
                        cell.sequence.store(pos + 1, Ordering::Release);
                        return Ok(());
                    }
                    Err(actual) => {
                        spin_hint();
                        pos = actual;
                    }
                }
            } else if dif < 0 {
                return Err(QueueError::Full);
            } else {
                spin_hint();
                pos = self.enqueue_pos.0.load(Ordering::Relaxed);
            }
        }
    }

    pub fn dequeue(&self) -> Result<T, QueueError> {
        let capacity = self.buffer.len();
        let mask = capacity - 1;
        let mut pos = self.dequeue_pos.0.load(Ordering::Relaxed);

        loop {
            let cell = &self.buffer[pos & mask];
            let seq = cell.sequence.load(Ordering::Acquire);

            let dif = (seq as isize) - ((pos + 1) as isize);
            if dif == 0 {
                // Same as `enqueue`: adopt the position the failed CAS reports
                // instead of retrying against a stale one.
                match self.dequeue_pos.0.compare_exchange_weak(
                    pos,
                    pos + 1,
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                ) {
                    Ok(_) => {
                        let data = cell.data.with(|slot| {
                            // SAFETY: Winning the CAS on `dequeue_pos` claims slot
                            // `pos & mask` exclusively. `sequence == pos + 1` was
                            // published with a `Release` store after the producer's
                            // `write`, and we observed it with an `Acquire` load, so
                            // the value is initialised and the write is visible.
                            // Reading moves the value out; the slot is only reused by
                            // a later `enqueue` after the `Release` store below.
                            unsafe { (*slot).assume_init_read() }
                        });
                        cell.sequence.store(pos + capacity, Ordering::Release);
                        return Ok(data);
                    }
                    Err(actual) => {
                        spin_hint();
                        pos = actual;
                    }
                }
            } else if dif < 0 {
                return Err(QueueError::Empty);
            } else {
                spin_hint();
                pos = self.dequeue_pos.0.load(Ordering::Relaxed);
            }
        }
    }
}

impl<T> Drop for LockFreeQueue<T> {
    fn drop(&mut self) {
        // C3: drain so every remaining `T` is dropped rather than leaked.
        while self.dequeue().is_ok() {}
    }
}

#[cfg(all(test, not(loom)))]
mod tests;

#[cfg(all(test, loom))]
mod loom_tests;
