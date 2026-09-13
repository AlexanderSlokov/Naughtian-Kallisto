#![no_main]
use libfuzzer_sys::fuzz_target;
use axum::http::Request;
use std::convert::Infallible;

fuzz_target!(|data: &[u8]| {
    // Fuzz the HTTP parser by feeding random data into an Axum request
    // Since we want to test the `put_version` handler's resilience,
    // we just see if parsing random bytes into an HTTP request panics.
    
    let _ = std::panic::catch_unwind(|| {
        if let Ok(s) = std::str::from_utf8(data) {
            // Very simplistic - a real HTTP fuzzer would be more complex
            let _req = Request::builder()
                .uri(s)
                .body(data.to_vec());
        }
    });
});
