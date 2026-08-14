use arc_swap::ArcSwap;
use std::collections::HashMap;
use std::sync::Arc;
use super::traits::SecretEngine;

pub struct EngineRegistry {
    engines: ArcSwap<HashMap<String, Arc<dyn SecretEngine>>>,
    write_lock: parking_lot::Mutex<()>,
}

impl Default for EngineRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl EngineRegistry {
    pub fn new() -> Self {
        Self {
            engines: ArcSwap::from_pointee(HashMap::new()),
            write_lock: parking_lot::Mutex::new(()),
        }
    }

    pub fn mount(&self, prefix: &str, engine: Arc<dyn SecretEngine>) {
        let _guard = self.write_lock.lock();
        let current = self.engines.load();
        let mut new_map = (**current).clone();
        new_map.insert(prefix.to_string(), engine);
        self.engines.store(Arc::new(new_map));
    }

    pub fn resolve(&self, prefix: &str) -> Option<Arc<dyn SecretEngine>> {
        let current = self.engines.load();
        current.get(prefix).cloned()
    }

    pub fn mounted_prefixes(&self) -> Vec<String> {
        let current = self.engines.load();
        current.keys().cloned().collect()
    }

    pub async fn flush_all(&self) {
        let current = self.engines.load();
        for engine in current.values() {
            let _ = engine.force_flush().await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::error::EngineError;
    use crate::engine::traits::{KeyMetadata, SecretPayload};
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicBool, Ordering};

    struct MockEngine {
        engine_type_str: &'static str,
        flush_called: Arc<AtomicBool>,
    }

    #[async_trait]
    impl SecretEngine for MockEngine {
        async fn read_version(&self, _path: &str, _version: u32) -> Result<(SecretPayload, crate::engine::traits::VersionState), EngineError> {
            Err(EngineError::NotFound)
        }
        async fn read_metadata(&self, _path: &str) -> Result<KeyMetadata, EngineError> {
            Err(EngineError::NotFound)
        }
        async fn put_version(&self, _path: &str, _payload: &SecretPayload, _cas: Option<u32>) -> Result<(), EngineError> {
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
            self.engine_type_str
        }
        async fn force_flush(&self) -> Result<(), EngineError> {
            self.flush_called.store(true, Ordering::Relaxed);
            Ok(())
        }
    }

    #[test]
    fn test_basic_mount_and_resolve() {
        let registry = EngineRegistry::new();
        let flush_called = Arc::new(AtomicBool::new(false));
        let mock = Arc::new(MockEngine {
            engine_type_str: "mock",
            flush_called,
        });

        registry.mount("secret", mock.clone());

        let resolved = registry.resolve("secret");
        assert!(resolved.is_some());
        assert_eq!(resolved.unwrap().engine_type(), "mock");

        let resolved_none = registry.resolve("non_existent");
        assert!(resolved_none.is_none());

        let prefixes = registry.mounted_prefixes();
        assert_eq!(prefixes.len(), 1);
        assert_eq!(prefixes[0], "secret");
    }

    #[tokio::test]
    async fn test_flush_all() {
        let registry = EngineRegistry::new();
        let flush_called1 = Arc::new(AtomicBool::new(false));
        let mock1 = Arc::new(MockEngine {
            engine_type_str: "mock1",
            flush_called: flush_called1.clone(),
        });

        let flush_called2 = Arc::new(AtomicBool::new(false));
        let mock2 = Arc::new(MockEngine {
            engine_type_str: "mock2",
            flush_called: flush_called2.clone(),
        });

        registry.mount("e1", mock1);
        registry.mount("e2", mock2);

        registry.flush_all().await;

        assert!(flush_called1.load(Ordering::Relaxed));
        assert!(flush_called2.load(Ordering::Relaxed));
    }

    #[test]
    fn test_overwrite_mount() {
        let registry = EngineRegistry::new();
        let flush_called1 = Arc::new(AtomicBool::new(false));
        let mock1 = Arc::new(MockEngine {
            engine_type_str: "mock1",
            flush_called: flush_called1,
        });

        let flush_called2 = Arc::new(AtomicBool::new(false));
        let mock2 = Arc::new(MockEngine {
            engine_type_str: "mock2",
            flush_called: flush_called2,
        });

        registry.mount("secret", mock1);
        registry.mount("secret", mock2);

        let resolved = registry.resolve("secret");
        assert!(resolved.is_some());
        assert_eq!(resolved.unwrap().engine_type(), "mock2");

        let prefixes = registry.mounted_prefixes();
        assert_eq!(prefixes.len(), 1);
    }
}

