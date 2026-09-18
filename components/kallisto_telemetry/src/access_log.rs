//! The access log: drop-on-full, never in the way of a read.
//!
//! ADR-0015 D15 draws the line this file sits on. An **audit** log records
//! before it serves, so a full queue means refusing to serve. An **access** log
//! records after the fact and drops when it cannot keep up, so a flood costs
//! log lines and not availability. Kallisto has the second one, and the ADR is
//! blunt about why the distinction must never blur: believing you can answer
//! "who read the Stripe key" when you cannot is worse than knowing you cannot.
//!
//! Three properties follow, and each is tested:
//!
//! * **A full queue drops.** It never blocks, never retries, never allocates a
//!   spill buffer. The count of what was lost is a metric, so an operator can
//!   be paged on it (QĐ-8) rather than discovering the gap later.
//! * **Every line carries a sequence number and a timestamp taken at the moment
//!   it was created.** ADR-0016 QĐ-3 gives each worker its own producer, so
//!   arrival order at the writer is not creation order.
//! * **Nothing identifying is written in the clear.** Paths and tokens go
//!   through [`crate::redact`]; values never appear at all.

use std::{
    io::Write,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};

use kallisto_queue::LockFreeQueue;

use crate::redact::Id;

/// Room for every field at its longest. Fixed so that a `Line` is `Copy`, the
/// queue's nodes are one size, and building one allocates nothing.
const LINE_CAPACITY: usize = 192;

/// What one served request contributes to the log.
///
/// Rendered at creation rather than at write time. That costs a little work on
/// the read path, but it is the only way the timestamp and sequence number can
/// describe when the request *happened* rather than when the writer got to it.
#[derive(Clone, Copy)]
pub struct Line {
    bytes: [u8; LINE_CAPACITY],
    len: u8,
}

impl Line {
    pub fn as_str(&self) -> &str {
        std::str::from_utf8(&self.bytes[..usize::from(self.len)]).unwrap_or("")
    }
}

impl std::fmt::Debug for Line {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The facts about one request, before they are rendered.
///
/// `path` and `token` are already identifiers, not text: a caller that has not
/// hashed them cannot construct this.
pub struct Record<'a> {
    pub method: &'a str,
    /// `data`, `metadata`, `list`, `sys`, or `-` when the route resolved to
    /// nothing. Never a caller-supplied string: this is a small closed set, so
    /// it cannot smuggle a path into the log in the clear.
    pub action: &'a str,
    pub path: Id,
    pub token: Id,
    pub status: u16,
    pub enforced: bool,
}

/// One worker's end of the log.
///
/// Each worker holds its own, so the only thing two workers share is the queue
/// itself — which they touch with one CAS each (ADR-0016 QĐ-3).
pub struct Producer {
    queue: Arc<LockFreeQueue<Line>>,
    sequence: AtomicU64,
    dropped: AtomicU64,
    worker: usize,
    /// Off means record nothing at all, rather than enqueue into a queue no
    /// writer is draining. The difference matters: the second shape fills up
    /// and then counts every line as dropped, so switching the log off would
    /// raise `kallisto_access_log_dropped_total` — the alarm that is supposed
    /// to mean "something is flooding this process" (QĐ-8) — for the one
    /// reason that is not an incident.
    enabled: bool,
}

impl Producer {
    /// Renders and enqueues one line, or counts a drop. Never blocks.
    pub fn record(&self, record: &Record<'_>) {
        if !self.enabled {
            return;
        }
        let sequence = self.sequence.fetch_add(1, Ordering::Relaxed);
        let line = self.render(sequence, record);

        // A full queue is not an error to handle, it is the designed behaviour.
        // The read path has already finished; there is nothing here worth
        // waiting on, and waiting is the thing this design exists to refuse.
        if self.queue.enqueue(line).is_err() {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    fn render(&self, sequence: u64, record: &Record<'_>) -> Line {
        let mut out = Writer::new();
        out.str("ts=");
        out.u64(now_millis());
        out.str(" seq=");
        out.u64(sequence);
        out.str(" worker=");
        out.u64(self.worker as u64);
        out.str(" method=");
        out.str(record.method);
        out.str(" action=");
        out.str(record.action);
        out.str(" path=");
        out.str(record.path.as_str());
        out.str(" token=");
        out.str(record.token.as_str());
        out.str(" status=");
        out.u64(u64::from(record.status));
        out.str(" authz=");
        out.str(if record.enforced { "on" } else { "off" });
        out.finish()
    }
}

/// Renders into the inline buffer, truncating rather than growing.
///
/// Truncation cannot happen with the fields above — [`LINE_CAPACITY`] is sized
/// past their maximum and a test pins that — but a silently half-written line
/// beats a panic on the read path if a future field is added carelessly.
struct Writer {
    bytes: [u8; LINE_CAPACITY],
    len: usize,
}

impl Writer {
    fn new() -> Self {
        Self {
            bytes: [0u8; LINE_CAPACITY],
            len: 0,
        }
    }

    fn str(&mut self, text: &str) {
        let room = LINE_CAPACITY - self.len;
        let take = text.len().min(room);
        self.bytes[self.len..self.len + take].copy_from_slice(&text.as_bytes()[..take]);
        self.len += take;
    }

    fn u64(&mut self, mut value: u64) {
        let mut digits = [0u8; 20];
        let mut at = digits.len();
        loop {
            at -= 1;
            digits[at] = b'0' + (value % 10) as u8;
            value /= 10;
            if value == 0 {
                break;
            }
        }
        // SAFETY-free: the bytes just written are ASCII digits.
        self.str(std::str::from_utf8(&digits[at..]).unwrap_or("0"));
    }

    fn finish(self) -> Line {
        Line {
            bytes: self.bytes,
            len: self.len as u8,
        }
    }
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// The shared queue, plus the handles the workers write through.
pub struct AccessLog {
    queue: Arc<LockFreeQueue<Line>>,
    producers: Vec<Arc<Producer>>,
    running: Arc<AtomicBool>,
}

impl AccessLog {
    /// `capacity` is rounded up to a power of two by the queue.
    pub fn new(capacity: usize, workers: usize) -> Self {
        Self::with_enabled(capacity, workers, true)
    }

    /// A log that records nothing. Not the same as one whose writer never runs
    /// — see [`Producer::enabled`].
    pub fn disabled(workers: usize) -> Self {
        Self::with_enabled(2, workers, false)
    }

    pub fn with_enabled(capacity: usize, workers: usize, enabled: bool) -> Self {
        let queue = Arc::new(LockFreeQueue::new(capacity.next_power_of_two().max(2)));
        let producers = (0..workers.max(1))
            .map(|worker| {
                Arc::new(Producer {
                    queue: Arc::clone(&queue),
                    sequence: AtomicU64::new(0),
                    dropped: AtomicU64::new(0),
                    worker,
                    enabled,
                })
            })
            .collect();
        Self {
            queue,
            producers,
            running: Arc::new(AtomicBool::new(true)),
        }
    }

    pub fn producer(&self, worker: usize) -> Arc<Producer> {
        Arc::clone(&self.producers[worker % self.producers.len()])
    }

    /// Lines lost to a full queue, across every worker.
    pub fn dropped(&self) -> u64 {
        self.producers.iter().map(|p| p.dropped()).sum()
    }

    /// Starts the single writer thread. Dropping the returned handle's owner
    /// and calling [`Self::stop`] drains what is left and ends the thread.
    ///
    /// A plain `std::thread`, not a tokio task: it does blocking writes and
    /// must never be scheduled onto a core that is answering requests.
    pub fn spawn_writer<W: Write + Send + 'static>(
        &self,
        mut sink: W,
    ) -> std::thread::JoinHandle<()> {
        let queue = Arc::clone(&self.queue);
        let running = Arc::clone(&self.running);

        std::thread::Builder::new()
            .name("kallisto-access-log".to_string())
            .spawn(move || {
                let mut idle: u32 = 0;
                loop {
                    let mut wrote = false;
                    // Batch: one `write_all` per drained run, not per line.
                    let mut batch = Vec::new();
                    while let Ok(line) = queue.dequeue() {
                        batch.extend_from_slice(line.as_str().as_bytes());
                        batch.push(b'\n');
                        wrote = true;
                        if batch.len() > 60_000 {
                            break;
                        }
                    }
                    if wrote {
                        let _ = sink.write_all(&batch);
                        let _ = sink.flush();
                        idle = 0;
                        continue;
                    }
                    if !running.load(Ordering::Relaxed) {
                        return;
                    }
                    // Back off to a sleep rather than spinning a core, and do
                    // it without a condvar: waking the writer through a mutex
                    // would put that mutex on the read path.
                    idle = (idle + 1).min(20);
                    std::thread::sleep(std::time::Duration::from_micros(u64::from(idle) * 50));
                }
            })
            .expect("the access log writer thread must start")
    }

    /// Asks the writer to drain and finish.
    pub fn stop(&self) {
        self.running.store(false, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::redact::LogKey;

    fn record<'a>(path: Id, token: Id) -> Record<'a> {
        Record {
            method: "GET",
            action: "data",
            path,
            token,
            status: 200,
            enforced: true,
        }
    }

    fn log(capacity: usize) -> AccessLog {
        AccessLog::new(capacity, 2)
    }

    #[test]
    fn a_line_carries_the_identifiers_and_never_the_originals() {
        let key = LogKey::random();
        let log = log(16);
        let producer = log.producer(0);
        producer.record(&record(key.id("app/db"), key.id("s.apptoken")));

        let line = log.queue.dequeue().unwrap();
        let text = line.as_str();
        assert!(!text.contains("app/db"), "{text}");
        assert!(!text.contains("s.apptoken"), "{text}");
        assert!(text.contains(key.id("app/db").as_str()), "{text}");
        assert!(text.contains("status=200"));
        assert!(text.contains("method=GET"));
    }

    /// The central promise of D15: the log gives up before the read path does.
    #[test]
    fn a_full_queue_drops_and_counts_instead_of_blocking() {
        let log = log(2);
        let producer = log.producer(0);
        for _ in 0..50 {
            producer.record(&record(Id::NONE, Id::NONE));
        }
        assert!(log.dropped() > 0, "a queue of 2 swallowed 50 lines");
        assert_eq!(log.dropped(), 50 - log.queue_depth_for_test());
    }

    /// QĐ-3 gives every worker its own producer, so the writer sees lines out
    /// of creation order. The sequence number is what puts them back.
    #[test]
    fn every_line_is_numbered_at_creation_in_its_own_producers_sequence() {
        let log = log(64);
        let first = log.producer(0);
        let second = log.producer(1);

        first.record(&record(Id::NONE, Id::NONE));
        second.record(&record(Id::NONE, Id::NONE));
        first.record(&record(Id::NONE, Id::NONE));

        let mut lines = Vec::new();
        while let Ok(line) = log.queue.dequeue() {
            lines.push(line.as_str().to_string());
        }
        assert!(lines[0].contains("seq=0") && lines[0].contains("worker=0"));
        assert!(lines[1].contains("seq=0") && lines[1].contains("worker=1"));
        assert!(lines[2].contains("seq=1") && lines[2].contains("worker=0"));
    }

    #[test]
    fn a_timestamp_is_taken_when_the_line_is_made() {
        let before = now_millis();
        let log = log(4);
        log.producer(0).record(&record(Id::NONE, Id::NONE));
        let after = now_millis();

        let line = log.queue.dequeue().unwrap();
        let ts: u64 = line
            .as_str()
            .strip_prefix("ts=")
            .and_then(|rest| rest.split(' ').next())
            .and_then(|t| t.parse().ok())
            .unwrap();
        assert!(
            (before..=after).contains(&ts),
            "{ts} not in {before}..{after}"
        );
    }

    /// If a line ever outgrew the buffer it would be silently truncated, and a
    /// truncated identifier aliases other paths. Pin the margin.
    #[test]
    fn the_longest_possible_line_fits_with_room_to_spare() {
        let log = log(4);
        let key = LogKey::random();
        log.producer(0).record(&Record {
            method: "DELETE",
            action: "metadata",
            path: key.id("a"),
            token: key.id("b"),
            status: 503,
            enforced: true,
        });
        let line = log.queue.dequeue().unwrap();
        assert!(line.as_str().ends_with("authz=on"), "{}", line.as_str());
        assert!(
            usize::from(line.len) < LINE_CAPACITY,
            "a full line used {} of {LINE_CAPACITY} bytes",
            line.len
        );
        // Both identifiers must survive whole: a truncated one aliases others.
        assert!(line.as_str().contains(key.id("a").as_str()));
        assert!(line.as_str().contains(key.id("b").as_str()));
    }

    /// Switching the log off must not look like a flood. With no writer
    /// running, enqueueing would fill the queue and then count every line as
    /// dropped — raising exactly the alarm that is supposed to mean somebody is
    /// hammering the process.
    #[test]
    fn a_disabled_log_records_nothing_and_drops_nothing() {
        let log = AccessLog::disabled(2);
        for _ in 0..100 {
            log.producer(0).record(&record(Id::NONE, Id::NONE));
            log.producer(1).record(&record(Id::NONE, Id::NONE));
        }
        assert_eq!(log.dropped(), 0);
        log.queue.dequeue().unwrap_err();
    }

    #[test]
    fn the_writer_drains_what_was_queued_and_then_stops() {
        let sink = Arc::new(Mutex::new(Vec::new()));
        let log = log(64);
        for _ in 0..5 {
            log.producer(0).record(&record(Id::NONE, Id::NONE));
        }

        let handle = log.spawn_writer(SharedSink(Arc::clone(&sink)));
        std::thread::sleep(std::time::Duration::from_millis(50));
        log.stop();
        handle.join().unwrap();

        let written = String::from_utf8(sink.lock().unwrap().clone()).unwrap();
        assert_eq!(written.lines().count(), 5, "{written}");
    }

    struct SharedSink(Arc<Mutex<Vec<u8>>>);
    impl Write for SharedSink {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl AccessLog {
        fn queue_depth_for_test(&self) -> u64 {
            let mut n = 0;
            while self.queue.dequeue().is_ok() {
                n += 1;
            }
            n
        }
    }
}
