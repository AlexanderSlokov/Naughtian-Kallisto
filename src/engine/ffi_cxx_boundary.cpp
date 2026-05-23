#include "kallisto/engine/ffi_cxx_boundary.hpp"
#include "kallisto/kallisto_core.hpp"

namespace kallisto::rust {

bool force_flush_engine(kallisto::KallistoCore* core) {
    if (core) {
        core->forceFlush();
        return true;
    }
    return false;
}

bool change_sync_mode_engine(kallisto::KallistoCore* core, int mode) {
    if (core) {
        // mode: 0 = IMMEDIATE, 1 = BATCH
        auto sync_mode = (mode == 1) ? KallistoCore::SyncMode::BATCH : KallistoCore::SyncMode::IMMEDIATE;
        core->changeSyncMode(sync_mode);
        return true;
    }
    return false;
}

} // namespace kallisto::rust
