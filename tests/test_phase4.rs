use naughtian_kallisto::KallistoCore;
use naughtian_kallisto::server::http_handler::AppState;
use naughtian_kallisto::event::worker::WorkerPool;
use std::sync::Arc;
use std::path::PathBuf;
use control_plane::admin_http::{start_admin_server, stop_admin_server};
use tokio::time::{sleep, Duration};
use reqwest::Client;
use serde_json::json;

#[tokio::test]
async fn test_phase4_integration() {
    let db_path = PathBuf::from("/tmp/kallisto_test_phase4");
    if db_path.exists() {
        std::fs::remove_dir_all(&db_path).unwrap();
    }

    let core = Arc::new(KallistoCore::new(db_path.to_str().unwrap()).unwrap());
    let state = AppState {
        registry: core.registry.clone(),
    };

    let data_port = 18200;
    let admin_port = 18202;

    let pool = WorkerPool::spawn(1, data_port, state.clone());
    let admin_server = start_admin_server(core.clone(), admin_port);

    sleep(Duration::from_millis(500)).await;

    let client = Client::new();

    // 1. Write secret
    let res = client
        .post(format!("http://127.0.0.1:{}/v1/secret/data/test_key", data_port))
        .json(&json!({"value": "super_secret", "ttl": 3600}))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success());

    // 2. Read secret
    let res = client
        .get(format!("http://127.0.0.1:{}/v1/secret/data/test_key", data_port))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success());
    let json: serde_json::Value = res.json().await.unwrap();
    assert_eq!(json["data"]["value"], "super_secret");

    // 3. Admin API: /admin/mode/immediate
    let res = client
        .post(format!("http://127.0.0.1:{}/admin/mode/immediate", admin_port))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success());

    // 4. Admin API: /admin/flush
    let res = client
        .post(format!("http://127.0.0.1:{}/admin/flush", admin_port)) // Actually we mapped to /admin/flush
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success());

    // 5. Admin API: /admin/mode/batch
    let res = client
        .post(format!("http://127.0.0.1:{}/admin/mode/batch", admin_port))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success());

    // 6. Soft-delete
    let res = client
        .post(format!("http://127.0.0.1:{}/v1/secret/delete/test_key", data_port))
        .json(&json!({"versions": [1]}))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success());

    // Read should fail
    let res = client
        .get(format!("http://127.0.0.1:{}/v1/secret/data/test_key", data_port))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), reqwest::StatusCode::NOT_FOUND);

    // 7. Undelete
    let res = client
        .post(format!("http://127.0.0.1:{}/v1/secret/undelete/test_key", data_port))
        .json(&json!({"versions": [1]}))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success());

    // Read should succeed
    let res = client
        .get(format!("http://127.0.0.1:{}/v1/secret/data/test_key", data_port))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success());

    // 8. Destroy
    let res = client
        .put(format!("http://127.0.0.1:{}/v1/secret/destroy/test_key", data_port))
        .json(&json!({"versions": [1]}))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success());

    // Read should fail
    let res = client
        .get(format!("http://127.0.0.1:{}/v1/secret/data/test_key", data_port))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), reqwest::StatusCode::NOT_FOUND);

    // Clean up
    stop_admin_server(admin_server);
}
