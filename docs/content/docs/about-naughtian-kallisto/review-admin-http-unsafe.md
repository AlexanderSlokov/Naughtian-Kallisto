---
title: "Deep Review: Unsafe Analysis & Safe Redesign"
weight: 20
---

# Deep Review: Unsafe Analysis & Safe Redesign for `admin_http.rs`

This report provides a production-critical, deep review of the unsafe code patterns inside the legacy FFI bridge `components/kallisto_cluster/src/admin_http.rs`. It evaluates the risks and outlines an elegant, 100% safe Rust redesign enabled by our pure Rust rewrite of the data plane.

---

### 1. Problem Summary
Historically, Kallisto used a hybrid C++/Rust architecture where the high-performance Data Plane was written in C++20, and the Gossip/Control Plane was written in Rust. 

The `admin_http.rs` file was designed as an **FFI adapter bridge** to allow a Rust-based Axum HTTP admin server to trigger administrative actions (like flushing to disk or changing sync mode) inside the C++ runtime. 

To achieve this cross-language control flow:
- C++ passed its raw internal core pointer (`*mut c_void`) to Rust.
- C++ passed unsafe C-style function pointers (`AdminCallbacks`) to Rust.
- Rust had to bypass type safety, thread safety, and standard memory management guarantees using raw pointers and `unsafe` overrides.

With the **Pure Rust Rewrite** (completing Phase 1/2), the legacy C++ data plane is completely replaced by a native, safe, and thread-safe Rust `KvEngine`. The need for FFI bridges, C callbacks, and raw pointers is **fully obsolete**.

---

### 2. Detailed Unsafe Code Analysis & Risks

Every single unsafe block in `admin_http.rs` introduces critical reliability, security, and maintenance risks:

#### A. Manual Thread Safety Overrides (Lines 11-15)
```rust
#[derive(Clone)]
pub struct SafeCorePointer(pub *mut std::ffi::c_void);

unsafe impl Send for SafeCorePointer {}
unsafe impl Sync for SafeCorePointer {}
```
- **Why it exists**: Raw pointers are not `Send` or `Sync` by default because the compiler cannot verify if multiple threads can safely read/write to the pointed-to memory.
- **The Risk**: This is a direct promise to the compiler ("Trust me, this pointer is safe to share across threads"). If the underlying C++ object is not thread-safe (e.g. if it mutates non-atomic state without synchronization), this triggers **undefined behavior (UB)**, data races, and memory corruption at runtime that the Rust compiler is blind to.

#### B. C-Style Unsafe Function Pointers (Lines 17-20)
```rust
pub struct AdminCallbacks {
    pub force_flush: unsafe fn(*mut std::ffi::c_void) -> bool,
    pub change_sync_mode: unsafe fn(*mut std::ffi::c_void, i32) -> bool,
}
```
- **Why it exists**: Enables calling back into C++ compiled code via raw function addresses.
- **The Risk**: Dereferencing arbitrary function pointers bypasses Rust's borrow checker and function signatures. If the C++ engine has already been dropped (use-after-free) or the pointer is corrupted, calling this will trigger a **Segmentation Fault** or jump execution to arbitrary memory locations (a primary vector for control-flow hijack exploits).

#### C. Manual Pointer Heap Management (Lines 81-102)
```rust
// In start_admin_server:
Box::into_raw(server)

// In stop_admin_server:
unsafe {
    let server = Box::from_raw(server_ptr);
    ...
}
```
- **Why it exists**: Bypasses Rust's automatic lifetime tracker (RAII) to pass ownership of a Rust structure (`AdminServer`) to C++ as a raw pointer.
- **The Risk**: Memory management is outsourced to C++. If C++ fails to call `stop_admin_server`, the memory leaks permanently. If C++ calls it twice, it triggers a **double-free error**, corrupting the heap and crashing the entire server.

#### D. Endpoints Dereferencing Core Pointer (Lines 104-129)
```rust
let success = unsafe { (state.callbacks.force_flush)(state.core_ptr.0) };
```
- **Why it exists**: Triggers the callback whenever an HTTP request lands on `/admin/save` or `/admin/mode/*`.
- **The Risk**: Highly vulnerable to concurrency bugs. If `/admin/save` is hammered with multiple concurrent requests, they all dereference the same raw C++ core pointer simultaneously, risking race conditions if the C++ side lacks strict internal mutexes.

---

### 3. Redesigning for 100% Safety (Zero Unsafe)

Because the data plane is now pure Rust, we can completely eliminate every trace of `unsafe` by using standard Rust traits, smart pointers, and async patterns.

#### Step 1: Define a Safe Control Trait
Instead of raw pointers and callbacks, we define a standard safe Rust trait representing engine administrative actions:

```rust
use crate::engine::error::EngineError;
use crate::engine::kv_engine::SyncMode;

pub trait EngineController: Send + Sync {
    fn force_flush(&self) -> Result<(), EngineError>;
    fn change_sync_mode(&self, mode: SyncMode);
}
```

We can easily implement this trait for `KvEngine` (or `EngineRegistry`):

```rust
impl EngineController for KvEngine {
    fn force_flush(&self) -> Result<(), EngineError> {
        // Safe RocksDB flush
        self.rocksdb.flush().map_err(|e| EngineError::StorageError(e.to_string()))
    }

    fn change_sync_mode(&self, mode: SyncMode) {
        self.change_sync_mode(mode);
    }
}
```

#### Step 2: Rewrite `AdminServer` safely
Using Axum with standard `Arc<dyn EngineController>` and async task coordination:

```rust
use axum::{
    extract::State,
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::sync::oneshot;

#[derive(Clone)]
struct AppState {
    engine: Arc<dyn EngineController>,
}

pub struct AdminServer {
    shutdown_tx: oneshot::Sender<()>,
}

impl AdminServer {
    pub async fn start(
        engine: Arc<dyn EngineController>,
        port: u16,
    ) -> Result<Self, std::io::Error> {
        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
        let state = AppState { engine };

        let app = Router::new()
            .route("/admin/save", post(handle_save))
            .route("/admin/mode/batch", post(handle_mode_batch))
            .route("/admin/mode/immediate", post(handle_mode_immediate))
            .route("/admin/status", get(handle_status))
            .with_state(state);

        let addr = format!("127.0.0.1:{}", port);
        let listener = tokio::net::TcpListener::bind(&addr).await?;
        println!("[Rust Admin Server] Listening on http://{}", addr);

        // Spawn Axum server directly onto the existing Tokio runtime
        tokio::spawn(async move {
            let server = axum::serve(listener, app)
                .with_graceful_shutdown(async move {
                    let _ = shutdown_rx.await;
                    println!("[Rust Admin Server] Shutdown signal received.");
                });
            if let Err(e) = server.await {
                eprintln!("[Rust Admin Server] Server error: {}", e);
            }
        });

        Ok(Self { shutdown_tx })
    }

    pub fn stop(self) {
        let _ = self.shutdown_tx.send(());
    }
}
```

#### Step 3: Implement Safe Endpoint Handlers
No `unsafe` blocks, fully structured errors, compile-time verified!

```rust
async fn handle_save(State(state): State<AppState>) -> (StatusCode, Json<Value>) {
    match state.engine.force_flush() {
        Ok(_) => (
            StatusCode::OK,
            Json(json!({"status": "OK", "message": "Database flushed to disk."})),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"errors": [format!("Failed to flush database: {}", e)]})),
        ),
    }
}

async fn handle_mode_batch(State(state): State<AppState>) -> (StatusCode, Json<Value>) {
    state.engine.change_sync_mode(SyncMode::Batch);
    (
        StatusCode::OK,
        Json(json!({"status": "OK", "message": "Mode changed to BATCH."})),
    )
}

async fn handle_mode_immediate(State(state): State<AppState>) -> (StatusCode, Json<Value>) {
    state.engine.change_sync_mode(SyncMode::Immediate);
    (
        StatusCode::OK,
        Json(json!({"status": "OK", "message": "Mode changed to IMMEDIATE."})),
    )
}

async fn handle_status() -> (StatusCode, Json<Value>) {
    (
        StatusCode::OK,
        Json(json!({
            "status": "OK",
            "version": env!("CARGO_PKG_VERSION"),
            "features": {
                "control_plane_port": 8202,
                "data_plane_port": 8200
            }
        })),
    )
}
```

---

### 4. Cost and Risk Assessment

| Aspect | Legacy Hybrid (Unsafe) | Proposed Safe Redesign | Improvement |
|---|---|---|---|
| **Correctness** | High risk of UB due to developer thread-safety guarantees. | 100% verified at compile-time by Rust borrow checker. | **Absolute Safety** |
| **Security** | Vulnerable to buffer overflows, jumps, and FFI memory exploits. | Immune to buffer exploits, secure sandboxed memory. | **Critical Fix** |
| **Robustness** | Memory leaks or double-frees if FFI calls desync. | Standard RAII guarantees automatic cleanup when dropped. | **Zero leaks** |
| **Cognitive Load** | High. Developers must handle raw pointer math and lifecycles. | Low. Standard idiomatic async Rust using standard traits. | **Huge reduction** |
| **CPU / Performance**| Virtually identical. Calling Rust trait objects has negligible overhead (~2ns virtual call cost vs raw pointer call). | Virtually identical. Axum server operates natively without double runtime creation. | **Parity** |

---

### 5. Recommendation
**Yes, we can and absolutely should eliminate `unsafe` completely.** 

By wrapping the engine control behind a clean, safe `EngineController` trait and passing it via `Arc<dyn EngineController>` directly to Axum:
1. We achieve **0 lines of unsafe code** in the entire cluster admin component.
2. We eliminate the redundant background thread spawning a duplicate Tokio runtime (we just spawn Axum tasks directly on the host's existing Tokio runtime).
3. We secure Kallisto's administrative APIs against double-frees, memory leaks, and FFI crashes.

This refactoring can be cleanly executed in **Phase 4 (Control Plane & Telemetry Migration)** of our rewrite plan.
