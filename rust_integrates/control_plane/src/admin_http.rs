use axum::{
    routing::{get, post},
    Router, Json, http::StatusCode,
    extract::State,
};
use serde_json::{json, Value};
use tokio::sync::oneshot;
use std::thread;
use std::sync::Arc;

#[derive(Clone)]
pub struct SafeCorePointer(pub *mut std::ffi::c_void);

unsafe impl Send for SafeCorePointer {}
unsafe impl Sync for SafeCorePointer {}

pub struct AdminCallbacks {
    pub force_flush: unsafe fn(*mut std::ffi::c_void) -> bool,
    pub change_sync_mode: unsafe fn(*mut std::ffi::c_void, i32) -> bool,
}

pub struct AdminServer {
    shutdown_tx: oneshot::Sender<()>,
    thread_handle: thread::JoinHandle<()>,
}

#[derive(Clone)]
struct AppState {
    core_ptr: SafeCorePointer,
    callbacks: Arc<AdminCallbacks>,
}

pub fn start_admin_server(
    core_ptr: *mut std::ffi::c_void,
    port: u16,
    callbacks: AdminCallbacks,
) -> *mut AdminServer {
    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
    let state = AppState {
        core_ptr: SafeCorePointer(core_ptr),
        callbacks: Arc::new(callbacks),
    };

    let thread_handle = thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("Failed to build Tokio runtime");

        rt.block_on(async {
            let app = Router::new()
                .route("/admin/save", post(handle_save))
                .route("/admin/mode/batch", post(handle_mode_batch))
                .route("/admin/mode/immediate", post(handle_mode_immediate))
                .route("/admin/status", get(handle_status))
                .with_state(state);

            let addr = format!("127.0.0.1:{}", port);
            println!("[Rust Admin Server] Listening on http://{}", addr);

            let listener = match tokio::net::TcpListener::bind(&addr).await {
                Ok(l) => l,
                Err(e) => {
                    eprintln!("[Rust Admin Server] Bind failed: {}", e);
                    return;
                }
            };

            let server = axum::serve(listener, app)
                .with_graceful_shutdown(async move {
                    let _ = shutdown_rx.await;
                    println!("[Rust Admin Server] Shutdown signal received.");
                });

            if let Err(e) = server.await {
                eprintln!("[Rust Admin Server] Server error: {}", e);
            }
        });
    });

    let server = Box::new(AdminServer {
        shutdown_tx,
        thread_handle,
    });

    Box::into_raw(server)
}

pub fn stop_admin_server(server_ptr: *mut AdminServer) {
    if server_ptr.is_null() {
        return;
    }
    unsafe {
        let server = Box::from_raw(server_ptr);
        println!("[Rust Admin Server] Stopping Admin Server...");
        // Signal shutdown
        let _ = server.shutdown_tx.send(());
        // Wait for thread to finish
        let _ = server.thread_handle.join();
        println!("[Rust Admin Server] Admin Server stopped.");
    }
}

async fn handle_save(State(state): State<AppState>) -> (StatusCode, Json<Value>) {
    let success = unsafe { (state.callbacks.force_flush)(state.core_ptr.0) };
    if success {
        (StatusCode::OK, Json(json!({"status": "OK", "message": "Database flushed to disk."})))
    } else {
        (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"errors": ["Failed to flush database: core call failed"]})))
    }
}

async fn handle_mode_batch(State(state): State<AppState>) -> (StatusCode, Json<Value>) {
    let success = unsafe { (state.callbacks.change_sync_mode)(state.core_ptr.0, 1) };
    if success {
        (StatusCode::OK, Json(json!({"status": "OK", "message": "Mode changed to BATCH."})))
    } else {
        (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"errors": ["Failed to change mode: core call failed"]})))
    }
}

async fn handle_mode_immediate(State(state): State<AppState>) -> (StatusCode, Json<Value>) {
    let success = unsafe { (state.callbacks.change_sync_mode)(state.core_ptr.0, 0) };
    if success {
        (StatusCode::OK, Json(json!({"status": "OK", "message": "Mode changed to IMMEDIATE."})))
    } else {
        (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"errors": ["Failed to change mode: core call failed"]})))
    }
}

async fn handle_status() -> (StatusCode, Json<Value>) {
    (StatusCode::OK, Json(json!({
        "status": "OK",
        "version": env!("CARGO_PKG_VERSION"),
        "features": {
            "control_plane_port": 8202,
            "data_plane_port": 8200
        }
    })))
}
