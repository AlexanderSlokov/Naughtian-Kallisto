use std::{net::SocketAddr, thread::JoinHandle};

use axum::Router;

use crate::server::listener::bind_reuseport;

/// Manages a pool of Tokio single-threaded runtimes.
///
/// This replicates the thread-per-core architecture of the legacy C++ system,
/// bypassing the Tokio multi-thread scheduler's work-stealing overhead for
/// better predictable tail latency and cache-locality under extreme load.
pub struct WorkerPool {
    handles: Vec<JoinHandle<()>>,
}

impl WorkerPool {
    /// Spawns `num_workers` threads. Each thread runs a localized Tokio runtime
    /// and listens on the same SO_REUSEPORT bound socket.
    ///
    /// `make_router` is called once per worker, on that worker's own thread,
    /// and is given that worker's index. That is deliberate: anything a router
    /// builds or claims for itself — the rate limiter, the barrier's decryption
    /// buffer, and since M6 the access-log producer and the metrics counters —
    /// then belongs to one core and is never shared, which is the point of
    /// thread-per-core (ADR-0016 QĐ-3).
    ///
    /// The index is what lets a worker claim *its* slice of state that was
    /// allocated up front, rather than registering itself into something shared
    /// at startup.
    ///
    /// `pins[i]` is the logical CPU worker `i` is pinned to, as chosen by
    /// [`crate::event::cpu_plan`]. A shorter list than `num_workers` leaves the
    /// rest unpinned.
    pub fn spawn<F>(num_workers: usize, addr: SocketAddr, pins: &[usize], make_router: F) -> Self
    where
        F: Fn(usize) -> Router + Clone + Send + 'static,
    {
        let handles = (0..num_workers)
            .map(|worker_idx| {
                let make_router = make_router.clone();
                // Empty means nothing was worth pinning to; the worker then
                // runs wherever the scheduler puts it, which beats crowding
                // every worker onto one CPU. See `cpu_plan`.
                let pin = pins.get(worker_idx).copied();

                std::thread::Builder::new()
                    .name(format!("wrk:{worker_idx}"))
                    .spawn(move || {
                        if let Some(cpu) = pin {
                            core_affinity::set_for_current(core_affinity::CoreId { id: cpu });
                        }

                        // One runtime per worker, so no work-stealing and no
                        // cross-core synchronisation on the read path.
                        let rt = tokio::runtime::Builder::new_current_thread()
                            .enable_all()
                            .build()
                            .unwrap();

                        rt.block_on(async move {
                            // Every worker binds the same port; the kernel
                            // hands each new connection to one of them.
                            let std_listener = bind_reuseport(addr).expect("Failed to bind port");
                            std_listener.set_nonblocking(true).unwrap();

                            let listener = tokio::net::TcpListener::from_std(std_listener).unwrap();
                            let app = make_router(worker_idx);

                            axum::serve(listener, app).await.unwrap();
                        });
                    })
                    .unwrap()
            })
            .collect();

        Self { handles }
    }

    /// Block until all workers have finished (which normally means never,
    /// unless they crash or are signalled to stop).
    pub fn join_all(self) {
        for handle in self.handles {
            let _ = handle.join();
        }
    }
}
