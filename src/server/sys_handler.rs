use axum::{routing::get, Json, Router};
use serde_json::{json, Value};

/// Mock system endpoints representing Vault's sys/ api
pub fn router<S: Clone + Send + Sync + 'static>() -> Router<S> {
    Router::new()
        .route("/health", get(health_check))
        .route("/mounts", get(get_mounts))
        .route("/seal-status", get(seal_status))
        .route("/internal/ui/mounts/*path", get(get_internal_ui_mounts))
}

async fn health_check() -> Json<Value> {
    Json(json!({
        "initialized": true,
        "sealed": false,
        "standby": false,
        "performance_standby": false,
        "replication_performance_mode": "disabled",
        "replication_dr_mode": "disabled",
        "server_time_utc": 0,
        "version": "1.13.0",
        "cluster_name": "kallisto-cluster",
        "cluster_id": "mock-cluster-id"
    }))
}

async fn get_mounts() -> Json<Value> {
    Json(json!({
        "secret/": {
            "accessor": "kv_mock",
            "config": {
                "default_lease_ttl": 0,
                "force_no_cache": false,
                "max_lease_ttl": 0
            },
            "description": "key/value secret storage",
            "local": false,
            "options": {
                "version": "2"
            },
            "seal_wrap": false,
            "type": "kv",
            "uuid": "mock-uuid"
        }
    }))
}

async fn get_internal_ui_mounts() -> Json<Value> {
    Json(json!({
        "options": {
            "version": "2"
        },
        "path": "secret/",
        "type": "kv"
    }))
}

async fn seal_status() -> Json<Value> {
    Json(json!({
        "type": "shamir",
        "initialized": true,
        "sealed": false,
        "t": 1,
        "n": 1,
        "progress": 0,
        "nonce": "",
        "version": "1.13.0",
        "migration": false,
        "cluster_name": "kallisto-cluster",
        "cluster_id": "mock-cluster-id",
        "recovery_seal": false
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, http::Request};
    use tower::ServiceExt;

    #[tokio::test]
    async fn test_health_check() {
        let app = router();
        let response = app
            .oneshot(Request::builder().uri("/health").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), 200);
        let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body: Value = serde_json::from_slice(&body_bytes).unwrap();
        
        assert_eq!(body["initialized"], true);
        assert_eq!(body["sealed"], false);
        assert_eq!(body["standby"], false);
    }

    #[tokio::test]
    async fn test_mounts() {
        let app = router();
        let response = app
            .oneshot(Request::builder().uri("/mounts").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), 200);
        let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body: Value = serde_json::from_slice(&body_bytes).unwrap();
        
        assert_eq!(body["secret/"]["type"], "kv");
    }

    #[tokio::test]
    async fn test_seal_status() {
        let app = router();
        let response = app
            .oneshot(Request::builder().uri("/seal-status").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), 200);
        let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body: Value = serde_json::from_slice(&body_bytes).unwrap();
        
        assert_eq!(body["type"], "shamir");
        assert_eq!(body["sealed"], false);
    }
}
