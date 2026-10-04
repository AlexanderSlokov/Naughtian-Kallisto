//! ADR-0013 Group B: exhaustive interleaving checks for `LockFreeQueue`.
//!
//! Run with `make loom`. These test the real `LockFreeQueue` — the same code
//! the engine uses — via the `sync` shim, not a hand-copied replica.
//!
//! Every test here is fail-able; `docs/references/verification-status.md`
//! records the exact mutation that breaks each one.

use loom::{sync::Arc, thread};

use super::{LockFreeQueue, QueueError};

/// B1: an enqueued item is never lost and never dequeued twice.
///
/// One producer, one concurrent consumer. Whatever the interleaving, the item
/// is either in the consumer's hand or still in the queue — never both, never
/// neither.
#[test]
fn b1_item_neither_lost_nor_duplicated() {
    loom::model(|| {
        let q = Arc::new(LockFreeQueue::new(2));

        let producer = {
            let q = q.clone();
            thread::spawn(move || q.enqueue(7usize).unwrap())
        };
        let consumer = {
            let q = q.clone();
            thread::spawn(move || q.dequeue().ok())
        };

        producer.join().unwrap();
        let seen = consumer.join().unwrap();

        let mut leftover = Vec::new();
        while let Ok(v) = q.dequeue() {
            leftover.push(v);
        }

        match seen {
            Some(v) => {
                assert_eq!(v, 7);
                assert!(
                    leftover.is_empty(),
                    "B1: item was dequeued and also left in the queue: {leftover:?}"
                );
            }
            None => assert_eq!(
                leftover,
                vec![7],
                "B1: consumer saw nothing and the queue does not hold the item either"
            ),
        }
    });
}

/// B1: two concurrent producers into a capacity-2 queue. Every enqueue that
/// reported success must be recoverable, and nothing else may come out.
///
/// Runs unbounded (no `preemption_bound`): once `enqueue` stopped retrying
/// against a stale `pos`, the model became small enough to exhaust. ADR-0013
/// names `shuttle` as the fallback if that ever changes.
#[test]
fn b1_concurrent_producers_preserve_every_success() {
    loom::model(|| {
        let q = Arc::new(LockFreeQueue::new(2));

        let t1 = {
            let q = q.clone();
            thread::spawn(move || q.enqueue(1usize).is_ok())
        };
        let t2 = {
            let q = q.clone();
            thread::spawn(move || q.enqueue(2usize).is_ok())
        };

        let ok1 = t1.join().unwrap();
        let ok2 = t2.join().unwrap();

        let mut got = Vec::new();
        while let Ok(v) = q.dequeue() {
            got.push(v);
        }
        got.sort_unstable();

        let mut want = Vec::new();
        if ok1 {
            want.push(1usize);
        }
        if ok2 {
            want.push(2usize);
        }

        assert_eq!(
            got, want,
            "B1: dequeued set does not match the set of successful enqueues"
        );
    });
}

/// B1 from the consumers' side: two consumers race for one item. Exactly one
/// gets it and the other sees `Empty`. The producer models never put two
/// threads on `dequeue_pos`, so this is the one that drives the dequeue CAS
/// into its failure branch — where the stale-position livelock fix lives.
#[test]
fn b1_concurrent_consumers_take_an_item_once() {
    loom::model(|| {
        let q = Arc::new(LockFreeQueue::new(2));
        q.enqueue(7usize).unwrap();

        let consumers: Vec<_> = (0..2)
            .map(|_| {
                let q = q.clone();
                thread::spawn(move || q.dequeue())
            })
            .collect();
        let mut taken: Vec<_> = consumers.into_iter().map(|h| h.join().unwrap()).collect();
        taken.sort_by_key(Result::is_err);

        assert_eq!(taken, vec![Ok(7), Err(QueueError::Empty)]);
    });
}

/// B2: a full queue returns `Err(Full)`, and a rejected enqueue never
/// overwrites an unconsumed slot.
#[test]
fn b2_full_queue_rejects_without_overwriting() {
    loom::model(|| {
        let q = Arc::new(LockFreeQueue::new(2));
        q.enqueue(1usize).unwrap();
        q.enqueue(2usize).unwrap();

        let t1 = {
            let q = q.clone();
            thread::spawn(move || q.enqueue(30usize))
        };
        let t2 = {
            let q = q.clone();
            thread::spawn(move || q.enqueue(40usize))
        };

        assert_eq!(
            t1.join().unwrap(),
            Err(QueueError::Full),
            "B2: enqueue into a full queue must report Full"
        );
        assert_eq!(
            t2.join().unwrap(),
            Err(QueueError::Full),
            "B2: enqueue into a full queue must report Full"
        );

        assert_eq!(q.dequeue().unwrap(), 1, "B2: slot 0 was overwritten");
        assert_eq!(q.dequeue().unwrap(), 2, "B2: slot 1 was overwritten");
    });
}

/// B2: a consumer freeing a slot concurrently with a producer retrying must
/// hand over exactly one slot — the producer either fills the freed slot or is
/// told the queue is full, and the surviving items stay in FIFO order.
#[test]
fn b2_slot_handover_is_exact() {
    loom::model(|| {
        let q = Arc::new(LockFreeQueue::new(2));
        q.enqueue(1usize).unwrap();
        q.enqueue(2usize).unwrap();

        let producer = {
            let q = q.clone();
            thread::spawn(move || q.enqueue(3usize).is_ok())
        };
        let consumer = {
            let q = q.clone();
            thread::spawn(move || q.dequeue().unwrap())
        };

        let accepted = producer.join().unwrap();
        let first = consumer.join().unwrap();
        assert_eq!(first, 1, "B2: FIFO order broken");

        let mut rest = Vec::new();
        while let Ok(v) = q.dequeue() {
            rest.push(v);
        }
        let expected: Vec<usize> = if accepted { vec![2, 3] } else { vec![2] };
        assert_eq!(rest, expected, "B2: slot accounting is off");
    });
}
