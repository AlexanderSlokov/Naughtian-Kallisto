//! Single- and multi-threaded tests. These run under plain `cargo test` and
//! under `cargo miri test` (ADR-0013 C2/C3).

use super::*;

#[test]
fn single_thread_roundtrip() {
    let q = LockFreeQueue::new(4);
    q.enqueue(10u64).unwrap();
    q.enqueue(20).unwrap();
    assert_eq!(q.dequeue().unwrap(), 10);
    assert_eq!(q.dequeue().unwrap(), 20);
    q.dequeue().unwrap_err();
}

#[test]
fn full_queue_returns_error() {
    let q = LockFreeQueue::new(2);
    q.enqueue(1u32).unwrap();
    q.enqueue(2).unwrap();
    assert_eq!(q.enqueue(3), Err(QueueError::Full));
    // B2: rejecting the third item must not have clobbered either slot.
    assert_eq!(q.dequeue().unwrap(), 1);
    assert_eq!(q.dequeue().unwrap(), 2);
}

#[test]
fn drop_partially_filled_no_leak() {
    // C3: Miri tracks allocations. Dropping without draining must not leak the
    // remaining `String` allocations.
    let q = LockFreeQueue::new(4);
    q.enqueue(String::from("leak_check_1")).unwrap();
    q.enqueue(String::from("leak_check_2")).unwrap();
    // Only dequeue one; the other must be dropped by `Drop`.
    drop(q.dequeue().unwrap());
    drop(q);
}

#[test]
fn send_across_thread() {
    // C2: exercise `unsafe impl Send/Sync` by actually sharing the queue.
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

#[test]
#[should_panic(expected = "at least 2")]
fn capacity_one_is_rejected() {
    // Capacity 1 collapses the "filled" and "drained" sequence markers: the
    // queue would overwrite the unconsumed item and then spin forever in
    // `dequeue`. Guard it at construction rather than shipping a livelock.
    let _ = LockFreeQueue::<u64>::new(1);
}
