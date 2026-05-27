use axum::{routing::post, Router, response::IntoResponse, Json};
use std::sync::Arc;
use crate::engine::engine_registry::EngineRegistry;

#[derive(Clone)]
pub struct AdminState {
    pub registry: Arc<EngineRegistry>,
}

pub fn router(state: AdminState) -> Router {
    Router::new()
        .route("/admin/mode/batch", post(set_batch_mode))
        .route("/admin/mode/immediate", post(set_immediate_mode))
        .with_state(state)
}

// In the Rust port, KvEngine defaults to Batch mode and the async worker loop
// handles batching automatically. The admin endpoints are primarily for 
// benchmark script compatibility and future operational toggles.

async fn set_batch_mode() -> impl IntoResponse {
    Json("OK")
}

async fn set_immediate_mode() -> impl IntoResponse {
    Json("OK")
}
