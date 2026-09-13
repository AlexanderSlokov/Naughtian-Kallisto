/// Operations that can be applied to a KV-v2 key's metadata.
/// Each variant captures the minimal input for one state transition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KvOp {
    Put {
        payload_len: usize,
        cas: Option<u32>,
    },
    SoftDelete {
        version: u32,
    },
    Undelete {
        version: u32,
    },
    Destroy {
        version: u32,
    },
    UpdateMeta {
        max_versions: u32,
        cas_required: bool,
        delete_version_after_ms: u64,
    },
}
