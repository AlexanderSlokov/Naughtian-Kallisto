use naughtian_kallisto::engine::engine_registry::EngineRegistry;
use naughtian_kallisto::engine::kv_engine::KvEngine;
use naughtian_kallisto::server::http_handler::AppState;
use naughtian_kallisto::event::worker::WorkerPool;
use std::sync::Arc;
use std::path::PathBuf;
use std::time::Duration;

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
    
    let pool = WorkerPool::spawn(num_workers, port, state);
    
    // In test environment, let it run until killed
    pool.join_all();
}
