use axum::{
    extract::State,
    http::{StatusCode, header, Uri},
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
use sonic_rs::JsonValueMutTrait;

#[derive(Clone)]
pub struct AppState {
    pub registry: Arc<EngineRegistry>,
}

pub fn vault_kv_router(state: AppState) -> Router {
    Router::new()
        .route("/v1/:mount/data/*path", get(read_secret).post(write_secret).delete(delete_latest).patch(patch_secret))
        .route("/v1/:mount/subkeys/*path", get(read_subkeys))
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
// Handlers & Extractors
// -----------------------------------------------------------------------------

fn extract_mount_and_path<'a>(uri_path: &'a str, expected_action: &str) -> Option<(&'a str, &'a str)> {
    let path_without_version = uri_path.strip_prefix("/v1/")?;
    let mut path_segments = path_without_version.splitn(3, '/');
    
    let mount = path_segments.next()?;
    let action = path_segments.next()?;
    
    if action != expected_action {
        return None;
    }
    
    let secret_path = path_segments.next()?;
    Some((mount, secret_path))
}

/// Extract ?version=N from URI query string without allocating a HashMap
#[inline]
fn extract_version_param(uri: &Uri) -> u32 {
    uri.query()
        .and_then(|q| {
            q.find("version=")
                .map(|i| &q[i + 8..])
                .and_then(|s| {
                    let end = s.find('&').unwrap_or(s.len());
                    s[..end].parse::<u32>().ok()
                })
        })
        .unwrap_or(0)
}

/// Extract ?depth=N from URI query string without allocating a HashMap
#[inline]
fn extract_depth_param(uri: &Uri) -> u32 {
    uri.query()
        .and_then(|q| {
            q.find("depth=")
                .map(|i| &q[i + 6..])
                .and_then(|s| {
                    let end = s.find('&').unwrap_or(s.len());
                    s[..end].parse::<u32>().ok()
                })
        })
        .unwrap_or(0)
}

/// Extract ?list=true from URI query string
#[inline]
fn extract_list_param(uri: &Uri) -> bool {
    uri.query()
        .map(|q| q.contains("list=true"))
        .unwrap_or(false)
}

async fn read_secret(
    State(state): State<AppState>,
    uri: Uri,
) -> Result<impl IntoResponse, AppError> {
    let (mount, path) = extract_mount_and_path(uri.path(), "data").ok_or(AppError::MountNotFound)?;
    let engine = state.registry.resolve(mount).ok_or(AppError::MountNotFound)?;
    
    let version = extract_version_param(&uri);
    let (payload, meta) = engine.read_version(path, version).await?;
    
    let created_time = crate::server::time_format::epoch_ms_to_rfc3339(meta.created_time_ms);
    let deletion_time = if meta.deletion_time_ms > 0 {
        format!("\"{}\"", crate::server::time_format::epoch_ms_to_rfc3339(meta.deletion_time_ms))
    } else {
        "\"\"".to_string()
    };
    let destroyed = if meta.destroyed { "true" } else { "false" };
    
    // Zero-allocation response construction
    let mut response = String::with_capacity(256 + payload.value.len());
    response.push_str(r#"{"data":{"data":"#);
    response.push_str(&payload.value);
    response.push_str(r#","metadata":{"version":"#);
    
    use std::fmt::Write;
    let _ = write!(&mut response, "{}", meta.version_id);
    
    response.push_str(r#","created_time":""#);
    response.push_str(&created_time);
    response.push_str(r#"","deletion_time":"#);
    response.push_str(&deletion_time);
    response.push_str(r#","destroyed":"#);
    response.push_str(destroyed);
    response.push_str(r#"}}}"#);
    
    Ok((StatusCode::OK, [(header::CONTENT_TYPE, "application/json")], response))
}

/// RFC 7396 JSON Merge Patch — in-place on sonic_rs::Value
#[inline]
fn json_merge_patch(target: &mut sonic_rs::Value, patch: &sonic_rs::Value) {
    if !patch.is_object() {
        *target = patch.clone();
        return; 
    }
    
    if !target.is_object() {
        *target = sonic_rs::json!({});
    }
    
    let patch_obj = patch.as_object().unwrap();
    let target_obj = target.as_object_mut().unwrap();
    
    for (key, value) in patch_obj.iter() {
        if value.is_null() {
            target_obj.remove(&key);
        } else {
            // Note: entry() and or_insert() are not available on sonic_rs::Object directly like this,
            // we have to check if it exists.
            if !target_obj.contains_key(&key) {
                target_obj.insert(&key, sonic_rs::json!({}));
            }
            json_merge_patch(target_obj.get_mut(&key).unwrap(), value);
        }
    }
}

async fn patch_secret(
    State(state): State<AppState>,
    uri: Uri,
    body: Bytes,
) -> Result<impl IntoResponse, AppError> {
    let (mount, path) = extract_mount_and_path(uri.path(), "data").ok_or(AppError::MountNotFound)?;
    let engine = state.registry.resolve(mount).ok_or(AppError::MountNotFound)?;
    
    let (current_payload, _meta) = engine.read_version(path, 0).await?;
    
    let mut current_value = sonic_rs::from_str::<sonic_rs::Value>(&current_payload.value).unwrap_or_else(|_| sonic_rs::json!({}));
    
    let patch_body = sonic_rs::from_slice::<sonic_rs::Value>(body.as_ref())
        .map_err(|_| AppError::Engine(EngineError::StorageError("Invalid patch JSON".to_string())))?;
        
    let patch_data = patch_body.pointer(sonic_rs::pointer!["data"])
        .unwrap_or(&patch_body);
        
    json_merge_patch(&mut current_value, patch_data);
    
    let mut options_cas = None;
    if let Some(opts) = patch_body.pointer(sonic_rs::pointer!["options"]) {
        if let Some(cas) = opts.pointer(sonic_rs::pointer!["cas"]).and_then(|v| v.as_u64()) {
            options_cas = Some(cas as u32);
        }
    }
    
    let new_payload = SecretPayload {
        value: sonic_rs::to_string(&current_value).unwrap(),
        ttl: current_payload.ttl,
    };
    
    engine.put_version(path, &new_payload, options_cas).await?;
    
    // In Vault KV v2, PATCH returns the newly created version's metadata. 
    // We can just read the latest metadata to construct a valid response.
    let meta_after = engine.read_metadata(path).await?;
    let latest_vs = meta_after.versions.last().unwrap();
    let created_time = crate::server::time_format::epoch_ms_to_rfc3339(latest_vs.created_time_ms);
    
    let response = format!(
        r#"{{"data":{{"version":{},"created_time":"{}","deletion_time":"","destroyed":false}}}}"#,
        latest_vs.version_id, created_time
    );
    
    Ok((StatusCode::OK, [(header::CONTENT_TYPE, "application/json")], response))
}

fn strip_to_subkeys(value: &mut sonic_rs::Value, current_depth: u32, max_depth: u32) {
    if max_depth > 0 && current_depth >= max_depth {
        *value = sonic_rs::json!(null); 
        return;
    }
    if let Some(obj) = value.as_object_mut() {
        for (_key, val) in obj.iter_mut() {
            if val.is_object() {
                strip_to_subkeys(val, current_depth + 1, max_depth);
            } else {
                *val = sonic_rs::json!(null);
            }
        }
    } else {
        *value = sonic_rs::json!(null);
    }
}

async fn read_subkeys(
    State(state): State<AppState>,
    uri: Uri,
) -> Result<impl IntoResponse, AppError> {
    let (mount, path) = extract_mount_and_path(uri.path(), "subkeys").ok_or(AppError::MountNotFound)?;
    let engine = state.registry.resolve(mount).ok_or(AppError::MountNotFound)?;
    
    let version = extract_version_param(&uri);
    let depth = extract_depth_param(&uri);
    
    let (payload, _meta) = engine.read_version(path, version).await?;
    
    let mut value = sonic_rs::from_str::<sonic_rs::Value>(&payload.value)
        .unwrap_or_else(|_| sonic_rs::json!({}));
        
    strip_to_subkeys(&mut value, 0, depth);
    
    let mut response = String::with_capacity(128 + payload.value.len());
    response.push_str(r#"{"data":{"subkeys":"#);
    response.push_str(&sonic_rs::to_string(&value).unwrap());
    response.push_str(r#"}}"#);
    
    Ok((StatusCode::OK, [(header::CONTENT_TYPE, "application/json")], response))
}

async fn write_secret(
    State(state): State<AppState>,
    uri: Uri,
    body: Bytes,
) -> Result<impl IntoResponse, AppError> {
    let (mount, path) = extract_mount_and_path(uri.path(), "data").ok_or(AppError::MountNotFound)?;
    let engine = state.registry.resolve(mount).ok_or(AppError::MountNotFound)?;
    
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
    uri: Uri,
) -> Result<impl IntoResponse, AppError> {
    let (mount, path) = extract_mount_and_path(uri.path(), "data").ok_or(AppError::MountNotFound)?;
    let engine = state.registry.resolve(mount).ok_or(AppError::MountNotFound)?;
    engine.soft_delete(path, 0).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn soft_delete_versions(
    State(state): State<AppState>,
    uri: Uri,
    body: Bytes,
) -> Result<impl IntoResponse, AppError> {
    let (mount, path) = extract_mount_and_path(uri.path(), "delete").ok_or(AppError::MountNotFound)?;
    let engine = state.registry.resolve(mount).ok_or(AppError::MountNotFound)?;
    
    let versions = parse_versions_list(&body);
    for version in versions {
        engine.soft_delete(path, version).await?;
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn undelete_versions(
    State(state): State<AppState>,
    uri: Uri,
    body: Bytes,
) -> Result<impl IntoResponse, AppError> {
    let (mount, path) = extract_mount_and_path(uri.path(), "undelete").ok_or(AppError::MountNotFound)?;
    let engine = state.registry.resolve(mount).ok_or(AppError::MountNotFound)?;
    
    let versions = parse_versions_list(&body);
    for version in versions {
        engine.undelete(path, version).await?;
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn destroy_versions(
    State(state): State<AppState>,
    uri: Uri,
    body: Bytes,
) -> Result<impl IntoResponse, AppError> {
    let (mount, path) = extract_mount_and_path(uri.path(), "destroy").ok_or(AppError::MountNotFound)?;
    let engine = state.registry.resolve(mount).ok_or(AppError::MountNotFound)?;
    
    let versions = parse_versions_list(&body);
    for version in versions {
        engine.destroy_version(path, version).await?;
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn read_metadata(
    State(state): State<AppState>,
    uri: Uri,
) -> Result<impl IntoResponse, AppError> {
    let (mount, path) = extract_mount_and_path(uri.path(), "metadata").ok_or(AppError::MountNotFound)?;
    let engine = state.registry.resolve(mount).ok_or(AppError::MountNotFound)?;
    
    if extract_list_param(&uri) {
        let keys = engine.list_keys(path).await?;
        let mut response = String::with_capacity(64 + keys.len() * 20);
        response.push_str(r#"{"data":{"keys":["#);
        for (i, key) in keys.iter().enumerate() {
            if i > 0 {
                response.push(',');
            }
            response.push('"');
            response.push_str(key);
            response.push('"');
        }
        response.push_str(r#"]}}"#);
        return Ok((StatusCode::OK, [(header::CONTENT_TYPE, "application/json")], response));
    }
    
    let metadata = engine.read_metadata(path).await?;
    
    let mut response = String::with_capacity(512);
    response.push_str(r#"{"data":{"cas_required":"#);
    response.push_str(if metadata.cas_required { "true" } else { "false" });
    response.push_str(r#","current_version":"#);
    use std::fmt::Write;
    let _ = write!(&mut response, "{}", metadata.current_version);
    response.push_str(r#","max_versions":"#);
    let _ = write!(&mut response, "{}", metadata.max_versions);
    response.push_str(r#","delete_version_after":""#);
    response.push_str(&crate::server::time_format::ms_to_vault_duration(metadata.delete_version_after_ms));
    response.push_str(r#"","custom_metadata":"#);
    response.push_str(&sonic_rs::to_string(&metadata.custom_metadata).unwrap());
    response.push_str(r#","versions":{"#);
    
    for (i, v) in metadata.versions.iter().enumerate() {
        if i > 0 {
            response.push(',');
        }
        let _ = write!(&mut response, r#""{}":{{"#, v.version_id);
        response.push_str(r#""created_time":""#);
        response.push_str(&crate::server::time_format::epoch_ms_to_rfc3339(v.created_time_ms));
        response.push_str(r#"","deletion_time":"#);
        if v.deletion_time_ms > 0 {
            response.push_str(&crate::server::time_format::epoch_ms_to_rfc3339(v.deletion_time_ms));
        }
        response.push_str(r#"","destroyed":"#);
        response.push_str(if v.destroyed { "true" } else { "false" });
        response.push('}');
    }
    
    response.push_str(r#"}}}"#);
    
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
    #[inline(never)]
    #[cold]
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
        
        let mut body = String::with_capacity(32 + err_msg.len());
        body.push_str(r#"{"errors":[""#);
        body.push_str(&err_msg);
        body.push_str(r#"]}"#);
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
        async fn read_version(&self, _path: &str, _version: u32) -> Result<(SecretPayload, VersionState), EngineError> {
            self.read_called.store(true, Ordering::Relaxed);
            Ok((
                SecretPayload {
                    value: "mocked_value".to_string(),
                    ttl: 0,
                },
                crate::engine::traits::VersionState {
                    created_time_ms: 0,
                    deletion_time_ms: 0,
                    version_id: 1,
                    destroyed: false,
                }
            ))
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

    #[test]
    fn test_extract_mount_and_path() {
        // Normal scenarios
        let (mount, path) = extract_mount_and_path("/v1/secret/data/my/key", "data").unwrap();
        assert_eq!(mount, "secret");
        assert_eq!(path, "my/key");

        let (mount, path) = extract_mount_and_path("/v1/kv2/metadata/deep/path/key", "metadata").unwrap();
        assert_eq!(mount, "kv2");
        assert_eq!(path, "deep/path/key");

        // Action mismatch
        assert!(extract_mount_and_path("/v1/secret/delete/key", "data").is_none());

        // Invalid prefixes
        assert!(extract_mount_and_path("/v2/secret/data/key", "data").is_none());
        assert!(extract_mount_and_path("v1/secret/data/key", "data").is_none());
        assert!(extract_mount_and_path("/v1/secret", "data").is_none());

        // Boundary cases (missing path or missing action)
        assert!(extract_mount_and_path("/v1/secret/data", "data").is_none());
        assert!(extract_mount_and_path("/v1/", "data").is_none());
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
