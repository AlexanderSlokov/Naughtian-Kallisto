#![no_main]
use libfuzzer_sys::fuzz_target;
use naughtian_kallisto::engine::traits::{KeyMetadata, SecretPayload};
use rkyv::archived_root;

fuzz_target!(|data: &[u8]| {
    // Attempt to deserialize the bytes as KeyMetadata.
    // If the data is completely malformed, this should safely reject it or 
    // simply not crash. (Note: rkyv unchecked deserialization is unsafe,
    // but in a fuzzing context we want to ensure it doesn't cause UB like
    // wild pointers reading out of bounds that leads to a segfault).
    // The engine uses `unsafe { archived_root::<KeyMetadata>(data) }`, so we
    // fuzz exactly that path to catch memory safety issues.
    
    // We wrap it in a catch_unwind in case it panics, though UB might still
    // segfault the fuzzer (which is the goal of finding issues).
    let _ = std::panic::catch_unwind(|| {
        if data.len() >= 8 { // Need some minimum bytes for rkyv
            unsafe {
                let _meta = archived_root::<KeyMetadata>(data);
            }
        }
    });

    let _ = std::panic::catch_unwind(|| {
        if data.len() >= 8 {
            unsafe {
                let _payload = archived_root::<SecretPayload>(data);
            }
        }
    });
});
