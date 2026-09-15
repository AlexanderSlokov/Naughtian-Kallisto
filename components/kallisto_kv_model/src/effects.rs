/// Side-effects produced by `apply`. The engine executes these in order
/// against the I/O layer (cache, storage, path index). The model itself
/// never performs I/O.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    WriteVersion {
        version: u32,
    },
    DeleteVersion {
        version: u32,
    },
    WriteMeta,
    IndexPath,
    /// Trim oldest version: the engine must delete the payload at this version
    /// from storage and cache after the metadata write.
    TrimVersion {
        version: u32,
    },
}
