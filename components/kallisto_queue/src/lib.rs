//! Dmitry Vyukov's bounded MPMC lock-free queue.
//!
//! Extracted from `naughtian-kallisto` so that `loom` can model-check it
//! (ADR-0013 invariants B1/B2). `make loom` compiles with
//! `RUSTFLAGS="--cfg loom"`, which tokio and hyper-util also react to — tokio
//! drops `tokio::net` under that cfg and hyper-util then fails to build.
//! Keeping the queue in a crate whose dependency graph is `loom` and nothing
//! else is what makes the model checker usable at all.

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

/// `sequence - pos` when the slot at `pos` is empty and a producer holding
/// `pos` may fill it.
const EMPTY: usize = 0;
/// `sequence - pos` when the slot at `pos` is filled and a consumer holding
/// `pos` may read it.
const FILLED: usize = 1;

/// One slot. Aligned to a cache line so that a producer filling one slot and a
/// consumer draining its neighbour never contend for the same line.
#[repr(C, align(64))]
struct Node<T> {
    /// Whose turn this slot is. The whole protocol, for a slot reached at
    /// position `pos` on lap `n`:
    ///
    /// - `pos + EMPTY` — empty; the producer holding `pos` may fill it;
    /// - `pos + FILLED` — filled; the consumer holding `pos` may read it;
    /// - `pos + capacity` — drained; which is `EMPTY` for the producer one lap
    ///   later, at `pos + capacity`.
    sequence: AtomicUsize,
    data: UnsafeCell<MaybeUninit<T>>,
}

/// Keeps the producers' cursor and the consumers' cursor on separate cache
/// lines, so a CAS on one side does not invalidate the other side's line.
#[repr(align(64))]
struct CachePadded<T>(T);

/// Bounded MPMC queue: any number of threads may enqueue and dequeue at once,
/// and neither side ever blocks — a full queue and an empty one are both an
/// immediate `Err`.
///
/// Capacity must be a power of two and at least 2 (see [`Self::new`]).
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
///
/// # Thread safety
///
/// `Send` and `Sync` only when the payload is `Send`, because a value enqueued
/// on one thread is dropped or read on another:
///
/// ```compile_fail
/// fn shareable<Q: Send + Sync>() {}
/// shareable::<kallisto_queue::LockFreeQueue<std::rc::Rc<u8>>>();
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
    /// Capacity 1 is rejected even though it is a power of two: `pos + FILLED`
    /// and `pos + capacity` are then the same value, so a filled slot already
    /// reads as drained. A capacity-1 queue therefore accepts a second enqueue
    /// over the unconsumed first one (losing and leaking it) and then spins
    /// forever in `dequeue`. Found by the loom model in `loom_tests`.
    pub fn new(capacity: usize) -> Self {
        assert!(
            capacity.is_power_of_two() && capacity >= 2,
            "Capacity must be a power of 2 and at least 2, got {capacity}"
        );
        let buffer = (0..capacity)
            .map(|pos| Node {
                sequence: AtomicUsize::new(pos + EMPTY),
                data: UnsafeCell::new(MaybeUninit::uninit()),
            })
            .collect();

        Self {
            buffer,
            enqueue_pos: CachePadded(AtomicUsize::new(0)),
            dequeue_pos: CachePadded(AtomicUsize::new(0)),
        }
    }

    pub fn enqueue(&self, value: T) -> Result<(), QueueError> {
        let (pos, node) = self
            .claim(&self.enqueue_pos.0, EMPTY)
            .ok_or(QueueError::Full)?;
        node.data.with_mut(|slot| {
            // SAFETY: `claim` won the CAS on `enqueue_pos`, which gives this
            // thread slot `pos` exclusively, and `sequence == pos + EMPTY`
            // proves the previous occupant was already moved out by `dequeue`
            // (or the slot was never initialised), so this write does not
            // overwrite a live value and cannot double-drop. The pointer comes
            // from `UnsafeCell`, so it carries write provenance. The `Release`
            // store below publishes the write to whoever reads `sequence`.
            unsafe { (*slot).write(value) };
        });
        node.sequence.store(pos + FILLED, Ordering::Release);
        Ok(())
    }

    pub fn dequeue(&self) -> Result<T, QueueError> {
        let (pos, node) = self
            .claim(&self.dequeue_pos.0, FILLED)
            .ok_or(QueueError::Empty)?;
        let value = node.data.with(|slot| {
            // SAFETY: `claim` won the CAS on `dequeue_pos`, which gives this
            // thread slot `pos` exclusively. `sequence == pos + FILLED` was
            // published with a `Release` store after the producer's `write`,
            // and `claim` observed it with an `Acquire` load, so the value is
            // initialised and the write is visible. Reading moves the value
            // out; the slot is only reused by a later `enqueue` after the
            // `Release` store below.
            unsafe { (*slot).assume_init_read() }
        });
        node.sequence
            .store(pos + self.buffer.len(), Ordering::Release);
        Ok(value)
    }

    /// Takes the next position on one side of the queue, the producers' or the
    /// consumers'. Both sides run this same protocol; they differ only in
    /// which `sequence` value marks a slot as theirs, which is `ready`.
    ///
    /// `None` when that slot is not ready yet: full, seen from the producers'
    /// side; empty, seen from the consumers'.
    #[inline]
    fn claim(&self, cursor: &AtomicUsize, ready: usize) -> Option<(usize, &Node<T>)> {
        let mask = self.buffer.len() - 1;
        let mut pos = cursor.load(Ordering::Relaxed);
        loop {
            let node = &self.buffer[pos & mask];
            let lag = node.sequence.load(Ordering::Acquire) as isize - (pos + ready) as isize;
            if lag < 0 {
                return None;
            }
            if lag > 0 {
                // Another thread already took `pos`; catch up and try again.
                spin_hint();
                pos = cursor.load(Ordering::Relaxed);
                continue;
            }
            // On failure the CAS reports who actually holds the slot; adopt it.
            // Vyukov's C++ original gets this for free because
            // `compare_exchange_weak` updates `pos` by reference. Discarding it
            // and retrying with a stale `pos` spins against the winner's
            // not-yet-published `sequence` store — a livelock the loom model
            // reports as unbounded branching.
            match cursor.compare_exchange_weak(pos, pos + 1, Ordering::Relaxed, Ordering::Relaxed) {
                Ok(_) => return Some((pos, node)),
                Err(actual) => {
                    spin_hint();
                    pos = actual;
                }
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
