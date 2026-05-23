#pragma once

namespace kallisto {
    class KallistoCore;
}

namespace kallisto::rust {
    bool force_flush_engine(kallisto::KallistoCore* core);
    bool change_sync_mode_engine(kallisto::KallistoCore* core, int mode);
}
