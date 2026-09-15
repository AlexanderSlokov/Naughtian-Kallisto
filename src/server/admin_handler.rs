use std::sync::Arc;

use axum::{Json, Router, extract::State, response::IntoResponse, routing::post};

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

// KvEngine defaults to Batch mode (write-behind). These endpoints switch every
// mounted engine's durability mode, which the durability integration tests
// (ADR-0013 D1/D2) depend on: D1 needs Immediate to be reachable at all.
//
// They previously returned "OK" without touching the mode.

async fn set_batch_mode(State(state): State<AdminState>) -> impl IntoResponse {
    state.registry.set_sync_mode_all(false).await;
    Json("OK")
}

async fn set_immediate_mode(State(state): State<AdminState>) -> impl IntoResponse {
    state.registry.set_sync_mode_all(true).await;
    Json("OK")
}
