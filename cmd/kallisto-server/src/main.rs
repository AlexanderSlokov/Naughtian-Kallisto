use naughtian_kallisto::engine::engine_registry::EngineRegistry;
use naughtian_kallisto::engine::kv_engine::KvEngine;
use naughtian_kallisto::server::http_handler::AppState;
use naughtian_kallisto::event::worker::WorkerPool;
use std::sync::Arc;
use std::path::PathBuf;


fn main() {
    let registry = EngineRegistry::new();
    
    // Test database path
    let db_path = PathBuf::from("/tmp/kallisto_server_bench");
    if db_path.exists() {
        std::fs::remove_dir_all(&db_path).unwrap();
    }
    
    let engine = Arc::new(KvEngine::open(db_path.to_str().unwrap()).unwrap());
    registry.mount("secret", engine);
    
    let state = AppState {
        registry: Arc::new(registry),
    };
    
    let num_workers = 2;
    let port = 8200;
    
    println!("Starting Kallisto Rust Server on port 8200 with {} workers...", num_workers);
    
    let pool = WorkerPool::spawn(num_workers, port, state.clone());
    
    // Start Admin API on port 8202
    let admin_state = naughtian_kallisto::server::admin_handler::AdminState {
        registry: state.registry.clone(),
    };
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        rt.block_on(async {
            let admin_router = naughtian_kallisto::server::admin_handler::router(admin_state);
            let listener = tokio::net::TcpListener::bind("0.0.0.0:8202").await.unwrap();
            println!("Starting Admin API on port 8202...");
            axum::serve(listener, admin_router).await.unwrap();
        });
    });

    // In test environment, let it run until killed
    pool.join_all();
}
