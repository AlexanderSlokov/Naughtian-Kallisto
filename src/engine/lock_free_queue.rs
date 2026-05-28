use std::mem::MaybeUninit;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::ptr;

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

// Manual thread-safety guarantees as per PingCAP/TiKV unsafe philosophy.
// Since the queue is inherently safe for concurrent access via atomics, we can declare it Send/Sync.
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
                if self.enqueue_pos.0.compare_exchange_weak(
                    pos, pos + 1, Ordering::Relaxed, Ordering::Relaxed
                ).is_ok() {
                    unsafe {
                        // 1. Unsafe Block: Direct memory write, bypassing borrow checker for zero-cost queueing
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
                if self.dequeue_pos.0.compare_exchange_weak(
                    pos, pos + 1, Ordering::Relaxed, Ordering::Relaxed
                ).is_ok() {
                    let data = unsafe {
                        // 2. Unsafe Block: Direct memory read, extracting ownership without cloning
                        ptr::read(cell.data.as_ptr())
                    };
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
        while let Ok(_) = self.dequeue() {}
    }
}
