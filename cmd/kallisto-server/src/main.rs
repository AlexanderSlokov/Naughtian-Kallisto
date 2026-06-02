use naughtian_kallisto::KallistoCore;
use naughtian_kallisto::server::http_handler::AppState;
use naughtian_kallisto::event::worker::WorkerPool;
use std::sync::Arc;
use std::path::PathBuf;
use control_plane::admin_http::{start_admin_server, stop_admin_server};

#[cfg(not(target_env = "msvc"))]
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

fn main() {
    let db_path = PathBuf::from("/tmp/kallisto_server_bench");
    if db_path.exists() {
        std::fs::remove_dir_all(&db_path).unwrap();
    }
    
    let core = Arc::new(KallistoCore::new(db_path.to_str().unwrap()).unwrap());
    
    let state = AppState {
        registry: core.registry.clone(),
    };
    
    let num_workers = 2;
    let port = 8200;
    
    println!("Starting Kallisto Rust Server on port 8200 with {} workers...", num_workers);
    
    let pool = WorkerPool::spawn(num_workers, port, state.clone());
    
    // Start Admin API on port 8202 using control_plane
    let admin_server = start_admin_server(core.clone(), 8202);

    // In test environment, let it run until killed
    pool.join_all();
    
    stop_admin_server(admin_server);
}
