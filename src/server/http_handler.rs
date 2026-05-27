use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post, put},
    Json, Router,
};
use serde::Deserialize;
use std::sync::Arc;

use crate::engine::engine_registry::EngineRegistry;
use crate::engine::error::EngineError;
use crate::engine::traits::SecretPayload;
use super::sys_handler;

#[derive(Clone)]
pub struct AppState {
    pub registry: Arc<EngineRegistry>,
}

pub fn vault_kv_router(state: AppState) -> Router {
    Router::new()
        .route("/v1/:mount/data/*path", get(read_secret).post(write_secret).delete(delete_latest))
        .route("/v1/:mount/delete/*path", post(soft_delete_versions))
        .route("/v1/:mount/undelete/*path", post(undelete_versions))
        .route("/v1/:mount/destroy/*path", put(destroy_versions))
        .route("/v1/:mount/metadata/*path", get(read_metadata))
        .nest("/v1/sys", sys_handler::router::<AppState>())
        .with_state(state)
}

// -----------------------------------------------------------------------------
// Request / Response Models
// -----------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct WriteSecretReq {
    pub data: serde_json::Value,
    pub options: Option<WriteOptions>,
}

#[derive(Deserialize)]
pub struct WriteOptions {
    pub cas: Option<u32>,
}

#[derive(Deserialize)]
pub struct VersionsReq {
    pub versions: Vec<u32>,
}

// -----------------------------------------------------------------------------
// Handlers
// -----------------------------------------------------------------------------

async fn read_secret(
    State(state): State<AppState>,
    Path((mount, path)): Path<(String, String)>,
) -> Result<impl IntoResponse, AppError> {
    let engine = state.registry.resolve(&mount).ok_or(AppError::MountNotFound)?;
    let path = path.trim_start_matches('/');
    
    let payload = engine.read_version(path, 0).await?;
    
    let data_json: serde_json::Value = serde_json::from_str(&payload.value)
        .unwrap_or_else(|_| serde_json::json!({"value": payload.value}));
    
    Ok(Json(serde_json::json!({
        "data": data_json,
        "metadata": {
            "version": 1,
            "created_time": "2023-01-01T00:00:00Z"
        }
    })))
}

async fn write_secret(
    State(state): State<AppState>,
    Path((mount, path)): Path<(String, String)>,
    Json(req): Json<WriteSecretReq>,
) -> Result<impl IntoResponse, AppError> {
    let engine = state.registry.resolve(&mount).ok_or(AppError::MountNotFound)?;
    let path = path.trim_start_matches('/');
    
    let cas = req.options.and_then(|o| o.cas);
    let payload = SecretPayload {
        value: req.data.to_string(),
        ttl: 0,
    };
    engine.put_version(path, &payload, cas).await?;
    
    Ok(axum::http::StatusCode::NO_CONTENT)
}

async fn delete_latest(
    State(state): State<AppState>,
    Path((mount, path)): Path<(String, String)>,
) -> Result<impl IntoResponse, AppError> {
    let engine = state.registry.resolve(&mount).ok_or(AppError::MountNotFound)?;
    let path = path.trim_start_matches('/');
    
    // Soft delete latest version
    engine.soft_delete(path, 0).await?;
    
    Ok(StatusCode::NO_CONTENT)
}

async fn soft_delete_versions(
    State(state): State<AppState>,
    Path((mount, path)): Path<(String, String)>,
    Json(req): Json<VersionsReq>,
) -> Result<impl IntoResponse, AppError> {
    let engine = state.registry.resolve(&mount).ok_or(AppError::MountNotFound)?;
    let path = path.trim_start_matches('/');
    
    for version in req.versions {
        engine.soft_delete(path, version).await?;
    }
    
    Ok(StatusCode::NO_CONTENT)
}

async fn undelete_versions(
    State(state): State<AppState>,
    Path((mount, path)): Path<(String, String)>,
    Json(req): Json<VersionsReq>,
) -> Result<impl IntoResponse, AppError> {
    let engine = state.registry.resolve(&mount).ok_or(AppError::MountNotFound)?;
    let path = path.trim_start_matches('/');
    
    for version in req.versions {
        engine.undelete(path, version).await?;
    }
    
    Ok(StatusCode::NO_CONTENT)
}

async fn destroy_versions(
    State(state): State<AppState>,
    Path((mount, path)): Path<(String, String)>,
    Json(req): Json<VersionsReq>,
) -> Result<impl IntoResponse, AppError> {
    let engine = state.registry.resolve(&mount).ok_or(AppError::MountNotFound)?;
    let path = path.trim_start_matches('/');
    
    for version in req.versions {
        engine.destroy_version(path, version).await?;
    }
    
    Ok(StatusCode::NO_CONTENT)
}

async fn read_metadata(
    State(state): State<AppState>,
    Path((mount, path)): Path<(String, String)>,
) -> Result<impl IntoResponse, AppError> {
    let engine = state.registry.resolve(&mount).ok_or(AppError::MountNotFound)?;
    let path = path.trim_start_matches('/');
    
    let metadata = engine.read_metadata(path).await?;
    
    Ok(Json(metadata))
}

// -----------------------------------------------------------------------------
// Error Handling
// -----------------------------------------------------------------------------

pub enum AppError {
    MountNotFound,
    Engine(EngineError),
}

impl From<EngineError> for AppError {
    fn from(err: EngineError) -> Self {
        AppError::Engine(err)
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> axum::response::Response {
        let (status, err_msg) = match self {
            AppError::MountNotFound => (StatusCode::NOT_FOUND, "Mount path not found".to_string()),
            AppError::Engine(e) => match e {
                EngineError::NotFound => (StatusCode::NOT_FOUND, "Secret not found".to_string()),
                EngineError::CasMismatch { .. } => (StatusCode::CONFLICT, "CAS mismatch".to_string()),
                EngineError::Destroyed => (StatusCode::NOT_FOUND, "Key destroyed".to_string()),
                EngineError::StorageError(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg),
                _ => (StatusCode::INTERNAL_SERVER_ERROR, "Internal error".to_string()),
            },
        };
        
        let body = Json(serde_json::json!({
            "errors": [err_msg]
        }));
        
        (status, body).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::traits::{KeyMetadata, SecretEngine};
    use async_trait::async_trait;
    use axum::{body::Body, http::Request};
    use std::sync::atomic::{AtomicBool, Ordering};
    use tower::ServiceExt;

    struct MockEngine {
        read_called: Arc<AtomicBool>,
        write_called: Arc<AtomicBool>,
    }

    #[async_trait]
    impl SecretEngine for MockEngine {
        async fn read_version(&self, _path: &str, _version: u32) -> Result<SecretPayload, EngineError> {
            self.read_called.store(true, Ordering::Relaxed);
            Ok(SecretPayload {
                value: "mocked_value".to_string(),
                ttl: 0,
            })
        }
        async fn read_metadata(&self, _path: &str) -> Result<KeyMetadata, EngineError> {
            Ok(KeyMetadata::default())
        }
        async fn put_version(&self, _path: &str, _payload: &SecretPayload, _cas: Option<u32>) -> Result<(), EngineError> {
            self.write_called.store(true, Ordering::Relaxed);
            Ok(())
        }
        async fn soft_delete(&self, _path: &str, _version: u32) -> Result<(), EngineError> {
            Ok(())
        }
        async fn undelete(&self, _path: &str, _version: u32) -> Result<(), EngineError> {
            Ok(())
        }
        async fn destroy_version(&self, _path: &str, _version: u32) -> Result<(), EngineError> {
            Ok(())
        }
        async fn list_keys(&self, _prefix: &str) -> Result<Vec<String>, EngineError> {
            Ok(vec![])
        }
        fn engine_type(&self) -> &'static str {
            "mock"
        }
        async fn force_flush(&self) -> Result<(), EngineError> {
            Ok(())
        }
    }

    fn setup_app() -> (Router, Arc<AtomicBool>, Arc<AtomicBool>) {
        let registry = EngineRegistry::new();
        let read_called = Arc::new(AtomicBool::new(false));
        let write_called = Arc::new(AtomicBool::new(false));
        
        let mock = Arc::new(MockEngine {
            read_called: read_called.clone(),
            write_called: write_called.clone(),
        });
        
        registry.mount("secret", mock);
        let state = AppState { registry: Arc::new(registry) };
        (vault_kv_router(state), read_called, write_called)
    }

    #[tokio::test]
    async fn test_read_secret_success() {
        let (app, read_called, _) = setup_app();
        
        let response = app
            .oneshot(Request::builder().uri("/v1/secret/data/my/key").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), 200);
        assert!(read_called.load(Ordering::Relaxed));
        
        let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
        
        assert_eq!(body["data"]["value"], "mocked_value");
    }

    #[tokio::test]
    async fn test_write_secret_success() {
        let (app, _, write_called) = setup_app();
        
        let req_body = serde_json::json!({
            "data": {
                "value": "new_secret",
                "ttl": 3600
            }
        });
        
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/secret/data/my/key")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&req_body).unwrap()))
                    .unwrap()
            )
            .await
            .unwrap();

        assert_eq!(response.status(), 204);
        assert!(write_called.load(Ordering::Relaxed));
    }
    
    #[tokio::test]
    async fn test_mount_not_found() {
        let (app, _, _) = setup_app();
        
        let response = app
            .oneshot(Request::builder().uri("/v1/unknown/data/key").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), 404);
        
        let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
        
        assert_eq!(body["errors"][0], "Mount path not found");
    }
}
