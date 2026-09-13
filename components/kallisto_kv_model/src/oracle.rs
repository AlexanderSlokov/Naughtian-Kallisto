#![allow(dead_code)]
use std::collections::BTreeMap;

use crate::{
    apply::{KeyMetadata, apply},
    ops::KvOp,
};

/// BTreeMap-backed reference implementation for proptest model comparison.
/// Acts as the ultimate source of truth for KV-v2 state transitions.
#[derive(Debug, Default)]
pub struct Oracle {
    store: BTreeMap<String, KeyMetadata>,
}

impl Oracle {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn apply_op(&mut self, path: &str, op: KvOp, now_ms: u64) {
        let meta = self.store.entry(path.to_string()).or_default();

        if let Ok((new_meta, _effects)) = apply(meta, op, now_ms) {
            *meta = new_meta;
        }
    }

    pub fn get_meta(&self, path: &str) -> Option<&KeyMetadata> {
        self.store.get(path)
    }
}
