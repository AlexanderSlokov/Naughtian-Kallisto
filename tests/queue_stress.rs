//! Runtime B1 stress test for `LockFreeQueue`.
//!
//! Deliberately lives in the root crate rather than in `kallisto_queue`:
//! `make verify-miri` runs Miri over `-p kallisto_queue`, and 4 threads x 2000
//! items is far outside Miri's budget. ADR-0013 forbids
//! `#[cfg_attr(miri, ignore)]`, so the test is kept out of Miri's scope instead
//! of exempted inside it. Loom covers the exhaustive interleavings on a
//! capacity-2 queue; this covers real contention that loom's state space cannot
//! reach.

use std::{
    collections::HashSet,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use kallisto_queue::LockFreeQueue;

const PRODUCERS: u64 = 4;
const PER_PRODUCER: u64 = 2000;
const TOTAL: u64 = PRODUCERS * PER_PRODUCER;

#[test]
fn b1_mpmc_stress_no_loss_no_duplication() {
    let q = Arc::new(LockFreeQueue::new(1024));
    let drained: Arc<Mutex<Vec<u64>>> = Arc::new(Mutex::new(Vec::new()));
    // Shared tally so a consumer can tell "queue momentarily empty" from
    // "everything has been drained" without taking the mutex on every miss.
    let seen = Arc::new(AtomicU64::new(0));

    let mut producers = Vec::new();
    for p in 0..PRODUCERS {
        let q = q.clone();
        producers.push(std::thread::spawn(move || {
            for i in 0..PER_PRODUCER {
                // Retry on Full: back-pressure is expected, loss is not.
                while q.enqueue(p * PER_PRODUCER + i).is_err() {
                    std::hint::spin_loop();
                }
            }
        }));
    }

    let mut consumers = Vec::new();
    for _ in 0..2 {
        let q = q.clone();
        let drained = drained.clone();
        let seen = seen.clone();
        consumers.push(std::thread::spawn(move || {
            let mut local = Vec::new();
            // The deadline is a safety net against a hang, not the stop
            // condition: consumers exit as soon as the global tally is complete.
            let deadline = Instant::now() + Duration::from_secs(60);
            while seen.load(Ordering::Relaxed) < TOTAL {
                match q.dequeue() {
                    Ok(v) => {
                        local.push(v);
                        seen.fetch_add(1, Ordering::Relaxed);
                    }
                    Err(_) => {
                        if Instant::now() >= deadline {
                            break;
                        }
                        std::hint::spin_loop();
                    }
                }
            }
            drained.lock().unwrap().extend(local);
        }));
    }

    for h in producers.into_iter().chain(consumers) {
        h.join().unwrap();
    }

    let mut all = std::mem::take(&mut *drained.lock().unwrap());
    while let Ok(v) = q.dequeue() {
        all.push(v);
    }

    let unique: HashSet<u64> = all.iter().copied().collect();
    assert_eq!(
        all.len(),
        unique.len(),
        "B1: {} item(s) dequeued more than once",
        all.len() - unique.len()
    );
    assert_eq!(
        unique.len() as u64,
        TOTAL,
        "B1: {} item(s) lost",
        TOTAL - unique.len() as u64
    );
}
