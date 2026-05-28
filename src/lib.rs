pub mod server;
pub mod engine;
pub mod storage;
pub mod event;

use std::sync::Arc;
use crate::engine::engine_registry::EngineRegistry;
use crate::engine::kv_engine::{KvEngine, SyncMode};

#[derive(Clone)]
pub struct KallistoCore {
    pub registry: Arc<EngineRegistry>,
    pub default_kv: Arc<KvEngine>,
}

impl KallistoCore {
    pub fn new(db_path: &str) -> Result<Self, engine::error::EngineError> {
        let registry = Arc::new(EngineRegistry::new());
        let default_kv = Arc::new(KvEngine::open(db_path)?);
        registry.mount("secret", default_kv.clone());
        Ok(Self {
            registry,
            default_kv,
        })
    }

    pub fn change_sync_mode(&self, mode: SyncMode) {
        self.default_kv.change_sync_mode(mode);
    }

    pub async fn force_flush(&self) {
        self.registry.flush_all().await;
    }
}
