use axum::{
    routing::{get, post},
    Router, Json, http::StatusCode,
    extract::State,
};
use serde_json::{json, Value};
use tokio::sync::oneshot;
use std::thread;
use std::sync::Arc;
use naughtian_kallisto::KallistoCore;
use naughtian_kallisto::engine::kv_engine::SyncMode;

pub struct AdminServer {
    shutdown_tx: oneshot::Sender<()>,
    thread_handle: thread::JoinHandle<()>,
}

#[derive(Clone)]
pub struct AppState {
    pub core: Arc<KallistoCore>,
}

pub fn start_admin_server(
    core: Arc<KallistoCore>,
    port: u16,
) -> AdminServer {
    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
    let state = AppState {
        core,
    };

    let thread_handle = thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("Failed to build Tokio runtime");

        rt.block_on(async {
            let app = Router::new()
                .route("/admin/flush", post(handle_save))
                .route("/admin/mode/batch", post(handle_mode_batch))
                .route("/admin/mode/immediate", post(handle_mode_immediate))
                .route("/admin/status", get(handle_status))
                .with_state(state);

            let addr = format!("0.0.0.0:{}", port);
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

    AdminServer {
        shutdown_tx,
        thread_handle,
    }
}

pub fn stop_admin_server(server: AdminServer) {
    println!("[Rust Admin Server] Stopping Admin Server...");
    let _ = server.shutdown_tx.send(());
    let _ = server.thread_handle.join();
    println!("[Rust Admin Server] Admin Server stopped.");
}

async fn handle_save(State(state): State<AppState>) -> (StatusCode, Json<Value>) {
    state.core.force_flush().await;
    (StatusCode::OK, Json(json!({"status": "OK", "message": "Database flushed to disk."})))
}

async fn handle_mode_batch(State(state): State<AppState>) -> (StatusCode, Json<Value>) {
    state.core.change_sync_mode(SyncMode::Batch);
    (StatusCode::OK, Json(json!({"status": "OK", "message": "Mode changed to BATCH."})))
}

async fn handle_mode_immediate(State(state): State<AppState>) -> (StatusCode, Json<Value>) {
    state.core.change_sync_mode(SyncMode::Immediate);
    (StatusCode::OK, Json(json!({"status": "OK", "message": "Mode changed to IMMEDIATE."})))
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
