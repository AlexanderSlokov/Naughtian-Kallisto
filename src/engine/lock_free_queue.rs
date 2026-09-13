use std::{
    mem::MaybeUninit,
    ptr,
    sync::atomic::{AtomicUsize, Ordering},
};

/// Dmitry Vyukov's MPMC Lock-Free Queue.
/// Provides ultra-low latency lock-free message passing.
/// Eliminates OS context switches, mutexes, and cond_vars on the hot path.
/// Capacity must be a power of 2.

#[derive(Debug)]
pub enum QueueError {
    Full,
    Empty,
}

#[repr(C, align(64))]
struct Node<T> {
    sequence: AtomicUsize,
    data: MaybeUninit<T>,
}

#[repr(align(64))]
struct CachePadded<T>(T);

pub struct LockFreeQueue<T> {
    buffer: Box<[Node<T>]>,
    enqueue_pos: CachePadded<AtomicUsize>,
    dequeue_pos: CachePadded<AtomicUsize>,
}

// SAFETY: All shared state (`enqueue_pos`, `dequeue_pos`, `Node::sequence`) is
// accessed exclusively through `AtomicUsize` operations. `Node::data` is only
// written after a successful CAS on `enqueue_pos` and only read after a
// successful CAS on `dequeue_pos`, so at most one thread accesses a slot's data
// at any time. The `T: Send` bound guarantees the payload itself is safe to
// transfer across threads.
unsafe impl<T: Send> Send for LockFreeQueue<T> {}
unsafe impl<T: Send> Sync for LockFreeQueue<T> {}

impl<T> LockFreeQueue<T> {
    pub fn new(capacity: usize) -> Self {
        assert!(capacity.is_power_of_two(), "Capacity must be a power of 2");
        let mut buffer = Vec::with_capacity(capacity);
        for i in 0..capacity {
            buffer.push(Node {
                sequence: AtomicUsize::new(i),
                data: MaybeUninit::uninit(),
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
                if self
                    .enqueue_pos
                    .0
                    .compare_exchange_weak(pos, pos + 1, Ordering::Relaxed, Ordering::Relaxed)
                    .is_ok()
                {
                    // SAFETY: The CAS on `enqueue_pos` succeeded, so this thread
                    // has exclusive ownership of slot `pos & mask`. The slot's
                    // previous data was consumed by a prior `dequeue` (or was
                    // never initialized—`MaybeUninit`), so `ptr::write` does not
                    // double-drop. The subsequent `Release` store on `sequence`
                    // publishes the write to consumers.
                    unsafe {
                        ptr::write(cell.data.as_ptr() as *mut T, data);
                    }
                    cell.sequence.store(pos + 1, Ordering::Release);
                    return Ok(());
                }
            } else if dif < 0 {
                return Err(QueueError::Full);
            } else {
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
                if self
                    .dequeue_pos
                    .0
                    .compare_exchange_weak(pos, pos + 1, Ordering::Relaxed, Ordering::Relaxed)
                    .is_ok()
                {
                    // SAFETY: The CAS on `dequeue_pos` succeeded, so this thread
                    // has exclusive read access to slot `pos & mask`. The producer
                    // wrote valid data via `ptr::write` and published it with a
                    // `Release` store on `sequence` (observed by our `Acquire`
                    // load above). `ptr::read` moves the value out; the slot is
                    // then logically empty and will be reused by a future
                    // `enqueue` only after we advance `sequence` below.
                    let data = unsafe { ptr::read(cell.data.as_ptr()) };
                    cell.sequence.store(pos + capacity, Ordering::Release);
                    return Ok(data);
                }
            } else if dif < 0 {
                return Err(QueueError::Empty);
            } else {
                pos = self.dequeue_pos.0.load(Ordering::Relaxed);
            }
        }
    }
}

impl<T> Drop for LockFreeQueue<T> {
    fn drop(&mut self) {
        while self.dequeue().is_ok() {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_thread_roundtrip() {
        let q = LockFreeQueue::new(4);
        q.enqueue(10u64).unwrap();
        q.enqueue(20).unwrap();
        assert_eq!(q.dequeue().unwrap(), 10);
        assert_eq!(q.dequeue().unwrap(), 20);
        assert!(q.dequeue().is_err());
    }

    #[test]
    fn full_queue_returns_error() {
        let q = LockFreeQueue::new(2);
        q.enqueue(1u32).unwrap();
        q.enqueue(2).unwrap();
        assert!(matches!(q.enqueue(3), Err(QueueError::Full)));
    }

    #[test]
    fn drop_partially_filled_no_leak() {
        // Miri tracks allocations; dropping without dequeuing everything
        // must not leak. The Drop impl drains remaining items.
        let q = LockFreeQueue::new(4);
        q.enqueue(String::from("leak_check_1")).unwrap();
        q.enqueue(String::from("leak_check_2")).unwrap();
        // Only dequeue one; the other must be dropped cleanly.
        let _ = q.dequeue().unwrap();
        drop(q);
    }

    #[test]
    fn send_across_thread() {
        // C2: Verify Send/Sync soundness by moving the queue to another thread.
        let q = std::sync::Arc::new(LockFreeQueue::new(4));
        let q2 = q.clone();
        let handle = std::thread::spawn(move || {
            q2.enqueue(42u64).unwrap();
        });
        handle.join().unwrap();
        assert_eq!(q.dequeue().unwrap(), 42);
    }

    #[test]
    fn wrap_around_reuse() {
        // Fill, drain, refill to exercise slot reuse across sequence wrap.
        let q = LockFreeQueue::new(2);
        for round in 0..4u64 {
            q.enqueue(round * 10).unwrap();
            q.enqueue(round * 10 + 1).unwrap();
            assert_eq!(q.dequeue().unwrap(), round * 10);
            assert_eq!(q.dequeue().unwrap(), round * 10 + 1);
        }
    }
}

