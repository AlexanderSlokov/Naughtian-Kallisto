//! ADR-0013 V3: the read path's own parsers on untrusted input.
//!
//! No `catch_unwind` anywhere: a panic here is the finding, and swallowing it
//! would hide it from libFuzzer.
//!
//! Replaces `http_parser`, which fuzzed the deleted engine-era handler. The
//! interesting shapes are the same ones they always were — the query-string
//! extractors index into the query by byte offset after a `find`, so a
//! multi-byte UTF-8 boundary is the case that panics, and a panic in a worker
//! costs that worker's connection.

#![no_main]

use axum::http::{HeaderMap, HeaderValue, Method, Uri};
use libfuzzer_sys::fuzz_target;
use naughtian_kallisto::server::vault_api::fuzz_api;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };

    // URI path splitter, against every action name the router branches on.
    for action in ["data", "metadata", "subkeys", "delete", "undelete", "destroy"] {
        let _ = fuzz_api::extract_mount_and_path(text, action);
    }

    if let Ok(uri) = text.parse::<Uri>() {
        let _ = fuzz_api::version_param(&uri);
        let _ = fuzz_api::wants_list(&uri);
        let _ = fuzz_api::policy_path(&uri);
        for method in [Method::GET, Method::PUT, Method::DELETE] {
            let _ = fuzz_api::classify(&method, &uri);
        }
        // `LIST` is an extension method and takes a different path through
        // `Method`, which is worth reaching from the fuzzer rather than only
        // from the unit tests.
        if let Ok(list) = Method::from_bytes(b"LIST") {
            let _ = fuzz_api::classify(&list, &uri);
        }
    }

    // Token extraction, from both header spellings.
    let mut headers = HeaderMap::new();
    if let Ok(value) = HeaderValue::from_bytes(data) {
        headers.insert("x-vault-token", value.clone());
        headers.insert(axum::http::header::AUTHORIZATION, value);
    }
    let _ = fuzz_api::presented_token(&headers);
});
