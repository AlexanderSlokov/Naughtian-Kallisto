//! ADR-0013 V3: Kallisto's own request parsers on untrusted input.
//!
//! No `catch_unwind` anywhere: a panic here is the finding, and swallowing it
//! would hide it from libFuzzer. The previous version of this target built an
//! `http::Request` and dropped it, which fuzzed the `http` crate's URI parser
//! and none of Kallisto.

#![no_main]

use axum::http::Uri;
use libfuzzer_sys::fuzz_target;
use naughtian_kallisto::server::http_handler::fuzz_api;

fuzz_target!(|data: &[u8]| {
    // Body parser for the delete/undelete/destroy routes.
    let _ = fuzz_api::parse_versions_list(data);

    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };

    // URI path splitter, against every action name it branches on.
    for action in ["data", "metadata", "subkeys", "delete", "undelete", "destroy"] {
        let _ = fuzz_api::extract_mount_and_path(text, action);
    }

    // Query-string extractors. These index into the query by byte offset after
    // a `find`, so a multi-byte UTF-8 boundary is the interesting case.
    if let Ok(uri) = text.parse::<Uri>() {
        let _ = fuzz_api::extract_version_param(&uri);
        let _ = fuzz_api::extract_depth_param(&uri);
        let _ = fuzz_api::extract_list_param(&uri);
    }

    // RFC 7396 merge patch and subkey projection both recurse over
    // attacker-shaped JSON. Split the input so the fuzzer controls both sides.
    let (left, right) = data.split_at(data.len() / 2);
    if let (Ok(mut target), Ok(patch)) = (
        sonic_rs::from_slice::<sonic_rs::Value>(left),
        sonic_rs::from_slice::<sonic_rs::Value>(right),
    ) {
        fuzz_api::json_merge_patch(&mut target, &patch);
        fuzz_api::strip_to_subkeys(&mut target, 0, u32::from(data.first().copied().unwrap_or(1)));
    }
});
