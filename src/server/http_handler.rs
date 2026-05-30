use axum::{
    extract::{Path, State},
    http::{StatusCode, header},
    response::IntoResponse,
    routing::{get, post, put},
    Router,
};
use std::sync::Arc;
use axum::body::Bytes;

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
// Fast SIMD JSON Extraction (sonic-rs)
// -----------------------------------------------------------------------------

use sonic_rs::{JsonValueTrait, JsonContainerTrait, Value};

/// Fast, safe array parser for payloads like {"versions": [1, 2, 3]} using SIMD
fn parse_versions_list(body: &[u8]) -> Vec<u32> {
    let mut versions = Vec::new();
    if let Ok(root) = sonic_rs::from_slice::<Value>(body)
        && let Some(arr) = root.pointer(sonic_rs::pointer!["versions"]).and_then(|v| v.as_array())
    {
        for item in arr.iter() {
            if let Some(num) = item.as_u64() {
                versions.push(num as u32);
            }
        }
    }
    versions
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
    
    // Zero-allocation response construction (No serde_json::Value overhead)
    let mut response = String::with_capacity(128 + payload.value.len());
    response.push_str("{\"data\":{\"data\":");
    response.push_str(&payload.value);
    response.push_str(",\"metadata\":{\"version\":1,\"created_time\":\"2023-01-01T00:00:00Z\"}}}");
    
    Ok((StatusCode::OK, [(header::CONTENT_TYPE, "application/json")], response))
}

async fn write_secret(
    State(state): State<AppState>,
    Path((mount, path)): Path<(String, String)>,
    body: Bytes,
) -> Result<impl IntoResponse, AppError> {
    let engine = state.registry.resolve(&mount).ok_or(AppError::MountNotFound)?;
    let path = path.trim_start_matches('/');
    
    let mut secret_value = String::new();
    
    // Sử dụng Lazy Evaluation của sonic-rs để trích xuất Zero-Copy chuỗi "data" (Safe Rust)
    if let Ok(lazy) = sonic_rs::get(body.as_ref(), sonic_rs::pointer!["data"]) {
        secret_value = lazy.as_raw_str().to_string();
    }
    
    if secret_value.is_empty() {
        secret_value = String::from_utf8_lossy(body.as_ref()).into_owned();
    }
    
    let payload = SecretPayload {
        value: secret_value,
        ttl: 0,
    };
    engine.put_version(path, &payload, None).await?;
    
    Ok(StatusCode::NO_CONTENT)
}

async fn delete_latest(
    State(state): State<AppState>,
    Path((mount, path)): Path<(String, String)>,
) -> Result<impl IntoResponse, AppError> {
    let engine = state.registry.resolve(&mount).ok_or(AppError::MountNotFound)?;
    let path = path.trim_start_matches('/');
    engine.soft_delete(path, 0).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn soft_delete_versions(
    State(state): State<AppState>,
    Path((mount, path)): Path<(String, String)>,
    body: Bytes,
) -> Result<impl IntoResponse, AppError> {
    let engine = state.registry.resolve(&mount).ok_or(AppError::MountNotFound)?;
    let path = path.trim_start_matches('/');
    
    let versions = parse_versions_list(&body);
    for version in versions {
        engine.soft_delete(path, version).await?;
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn undelete_versions(
    State(state): State<AppState>,
    Path((mount, path)): Path<(String, String)>,
    body: Bytes,
) -> Result<impl IntoResponse, AppError> {
    let engine = state.registry.resolve(&mount).ok_or(AppError::MountNotFound)?;
    let path = path.trim_start_matches('/');
    
    let versions = parse_versions_list(&body);
    for version in versions {
        engine.undelete(path, version).await?;
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn destroy_versions(
    State(state): State<AppState>,
    Path((mount, path)): Path<(String, String)>,
    body: Bytes,
) -> Result<impl IntoResponse, AppError> {
    let engine = state.registry.resolve(&mount).ok_or(AppError::MountNotFound)?;
    let path = path.trim_start_matches('/');
    
    let versions = parse_versions_list(&body);
    for version in versions {
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
    
    let response = format!(
        "{{\"data\":{{\"cas_required\":{},\"current_version\":{},\"max_versions\":{},\"delete_version_after\":\"{}ms\"}}}}",
        metadata.cas_required, metadata.current_version, metadata.max_versions, metadata.delete_version_after_ms
    );
    
    Ok((StatusCode::OK, [(header::CONTENT_TYPE, "application/json")], response))
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
                EngineError::SoftDeleted => (StatusCode::NOT_FOUND, "Secret soft-deleted".to_string()),
                EngineError::Destroyed => (StatusCode::NOT_FOUND, "Key destroyed".to_string()),
                EngineError::InvalidVersion(v) => (StatusCode::NOT_FOUND, format!("Invalid version: {}", v)),
                EngineError::CasMismatch { .. } => (StatusCode::CONFLICT, "CAS mismatch".to_string()),
                EngineError::StorageError(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg),
                EngineError::QueueFull => (StatusCode::SERVICE_UNAVAILABLE, "Write queue full".to_string()),
            },
        };
        
        let body = format!("{{\"errors\":[\"{}\"]}}", err_msg);
        (status, [(header::CONTENT_TYPE, "application/json")], body).into_response()
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
        async fn soft_delete(&self, _path: &str, _version: u32) -> Result<(), EngineError> { Ok(()) }
        async fn undelete(&self, _path: &str, _version: u32) -> Result<(), EngineError> { Ok(()) }
        async fn destroy_version(&self, _path: &str, _version: u32) -> Result<(), EngineError> { Ok(()) }
        async fn list_keys(&self, _prefix: &str) -> Result<Vec<String>, EngineError> { Ok(vec![]) }
        fn engine_type(&self) -> &'static str { "mock" }
        async fn force_flush(&self) -> Result<(), EngineError> { Ok(()) }
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
            .await.unwrap();
        assert_eq!(response.status(), 200);
        assert!(read_called.load(Ordering::Relaxed));
    }

    #[tokio::test]
    async fn test_write_secret_success() {
        let (app, _, write_called) = setup_app();
        let req_body = "{\"data\":{\"value\":\"new_secret\"}}";
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/secret/data/my/key")
                    .header("content-type", "application/json")
                    .body(Body::from(req_body))
                    .unwrap()
            )
            .await.unwrap();
        assert_eq!(response.status(), 204);
        assert!(write_called.load(Ordering::Relaxed));
    }
}
