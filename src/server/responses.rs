//! Vault-shaped JSON bodies, built by concatenation.
//!
//! Nothing here goes through `serde`. The values are already JSON — the
//! snapshot rendered them once when the file was adopted — so serving a secret
//! is `push_str` around a string that already exists. That is the whole reason
//! the read path is cheap, and it is why these builders live in one file where
//! the shapes can be checked against Vault's by eye.
//!
//! The shapes are Vault's, not ours: ADR-0015 D7 is a compatibility table, and
//! a field renamed here is an SDK that breaks in production.

use std::fmt::Write;

/// Vault reports its own version on `sys/health`, and more than one SDK and
/// more than one operator's dashboard reads it. Kallisto is not Vault, so it
/// also reports `kallisto_version` beside it; this field exists so that the
/// clients ADR-0015 D7 promises to keep working keep working.
pub const VAULT_COMPAT_VERSION: &str = "1.13.0";
pub const CLUSTER_NAME: &str = "kallisto";

/// Appends `value` as a quoted JSON string, escaping what RFC 8259 requires.
///
/// Secret *values* never pass through here — they are already-valid JSON,
/// written verbatim. This is for keys, paths and messages, which arrive from
/// the file and from the request.
pub fn push_json_string(out: &mut String, value: &str) {
    out.push('"');
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// `{"errors":["..."]}` — the shape every Vault client knows how to unwrap.
pub fn errors(message: &str) -> String {
    let mut out = String::with_capacity(16 + message.len());
    out.push_str(r#"{"errors":["#);
    push_json_string(&mut out, message);
    out.push_str("]}");
    out
}

/// `GET /v1/:mount/data/:path`.
///
/// `data` is spliced in as-is. `version` is the sealed file's content version
/// (ADR-0016 QĐ-2): Kallisto keeps no per-secret history, so every secret in a
/// given file reports that file's number.
pub fn kv_data(data: &str, version: u64, created_time: &str) -> String {
    let mut out = String::with_capacity(160 + data.len());
    out.push_str(
        r#"{"request_id":"","lease_id":"","renewable":false,"lease_duration":0,"data":{"data":"#,
    );
    out.push_str(data);
    out.push_str(r#","metadata":{"created_time":"#);
    push_json_string(&mut out, created_time);
    out.push_str(r#","custom_metadata":null,"deletion_time":"","destroyed":false,"version":"#);
    let _ = write!(&mut out, "{version}");
    out.push_str(r#"}},"wrap_info":null,"warnings":null,"auth":null}"#);
    out
}

/// `LIST /v1/:mount/metadata/:path`, and its `?list=true` spelling.
pub fn list_keys(keys: &[String]) -> String {
    let mut out = String::with_capacity(64 + keys.len() * 24);
    out.push_str(r#"{"data":{"keys":["#);
    for (i, key) in keys.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        push_json_string(&mut out, key);
    }
    out.push_str(r#"]}}"#);
    out
}

/// `GET /v1/:mount/metadata/:path`.
///
/// Exactly one entry in `versions`, for the same reason `kv_data` reports one
/// number: the history lives in git and in the bucket, not in this process.
pub fn kv_metadata(version: u64, created_time: &str) -> String {
    let mut out = String::with_capacity(320);
    out.push_str(r#"{"data":{"cas_required":false,"created_time":"#);
    push_json_string(&mut out, created_time);
    out.push_str(r#","current_version":"#);
    let _ = write!(&mut out, "{version}");
    out.push_str(
        r#","custom_metadata":null,"delete_version_after":"0s","max_versions":0,"oldest_version":"#,
    );
    let _ = write!(&mut out, "{version}");
    out.push_str(r#","updated_time":"#);
    push_json_string(&mut out, created_time);
    out.push_str(r#","versions":{""#);
    let _ = write!(&mut out, "{version}");
    out.push_str(r#"":{"created_time":"#);
    push_json_string(&mut out, created_time);
    out.push_str(r#","deletion_time":"","destroyed":false}}}}"#);
    out
}

/// `GET /v1/sys/health`.
///
/// The three `kallisto_` fields are the ones ADR-0015 D4 asks for: an operator
/// with three machines should be able to ask each one which file it is serving
/// and see immediately whether they agree.
pub fn health(
    sealed: bool,
    server_time_utc: u64,
    file_version: Option<u64>,
    etag: Option<&str>,
    loaded_at: Option<&str>,
    // `Some(false)` is the sidecar deployment of ADR-0015 D8, where the file
    // carries no token table and every read is permitted. That is a legitimate
    // configuration and a dangerous one to be in by accident, so it is reported
    // rather than left to be discovered.
    enforces: Option<bool>,
) -> String {
    let mut out = String::with_capacity(512);
    out.push_str(r#"{"initialized":true,"sealed":"#);
    out.push_str(if sealed { "true" } else { "false" });
    out.push_str(
        r#","standby":false,"performance_standby":false,"replication_performance_mode":"disabled","replication_dr_mode":"disabled","server_time_utc":"#,
    );
    let _ = write!(&mut out, "{server_time_utc}");
    out.push_str(r#","version":""#);
    out.push_str(VAULT_COMPAT_VERSION);
    out.push_str(r#"","cluster_name":""#);
    out.push_str(CLUSTER_NAME);
    out.push_str(r#"","cluster_id":"","kallisto_version":""#);
    out.push_str(env!("CARGO_PKG_VERSION"));
    out.push_str(r#"","kallisto_file_version":"#);
    match file_version {
        Some(v) => {
            let _ = write!(&mut out, "{v}");
        }
        None => out.push_str("null"),
    }
    out.push_str(r#","kallisto_etag":"#);
    match etag {
        Some(e) => push_json_string(&mut out, e),
        None => out.push_str("null"),
    }
    out.push_str(r#","kallisto_loaded_at":"#);
    match loaded_at {
        Some(t) => push_json_string(&mut out, t),
        None => out.push_str("null"),
    }
    out.push_str(r#","kallisto_authorization":"#);
    match enforces {
        Some(true) => out.push_str(r#""enforced""#),
        Some(false) => out.push_str(r#""none""#),
        None => out.push_str("null"),
    }
    out.push('}');
    out
}

/// `GET /v1/sys/seal-status`.
///
/// `type: shamir` is a lie of omission that ADR-0015 D7 signs off on: there is
/// no unseal ceremony, the key comes from the environment, and clients that
/// branch on this field expect one of Vault's own values.
pub fn seal_status(sealed: bool) -> String {
    let mut out = String::with_capacity(256);
    out.push_str(r#"{"type":"shamir","initialized":true,"sealed":"#);
    out.push_str(if sealed { "true" } else { "false" });
    out.push_str(r#","t":1,"n":1,"progress":0,"nonce":"","version":""#);
    out.push_str(VAULT_COMPAT_VERSION);
    out.push_str(r#"","migration":false,"cluster_name":""#);
    out.push_str(CLUSTER_NAME);
    out.push_str(r#"","cluster_id":"","recovery_seal":false,"storage_type":"kallisto"}"#);
    out
}

/// `GET /v1/sys/init`.
///
/// Always initialised. There is no unseal ceremony and no key-shard dance to be
/// part-way through: either a file has loaded or it has not, and `sys/health`
/// and `sys/seal-status` are where that shows.
///
/// This exists because an SDK asks for it before it asks for anything useful —
/// `hvac`'s `is_initialized()` is a `GET /v1/sys/init` — and a 404 there stops
/// a client on its first call. It was missing until the duck suite caught it,
/// which is the entire argument for testing against real SDKs rather than
/// against our own idea of what they send.
pub fn init_status() -> String {
    r#"{"initialized":true}"#.to_string()
}

/// `GET /v1/sys/mounts`. One mount, KV version 2, because that is all there is.
pub fn mounts(mount: &str) -> String {
    let mut out = String::with_capacity(320);
    out.push_str(r#"{"data":{"#);
    push_json_string(&mut out, &format!("{mount}/"));
    out.push_str(
        r#":{"accessor":"kv_kallisto","config":{"default_lease_ttl":0,"force_no_cache":false,"max_lease_ttl":0},"description":"key/value secret storage","local":false,"options":{"version":"2"},"seal_wrap":false,"type":"kv","uuid":"kallisto"}}}"#,
    );
    out
}

/// `GET /v1/sys/internal/ui/mounts/:path`. The SDKs call this to discover
/// whether a mount is KV v1 or v2 before choosing a URL shape.
pub fn ui_mount(mount: &str) -> String {
    let mut out = String::with_capacity(160);
    out.push_str(r#"{"data":{"options":{"version":"2"},"path":"#);
    push_json_string(&mut out, &format!("{mount}/"));
    out.push_str(r#","type":"kv"}}"#);
    out
}

/// `GET /v1/auth/token/lookup-self` and `POST /v1/auth/token/renew-self`.
///
/// Many SDKs call one of these on startup and refuse to proceed if it fails,
/// which is why ADR-0015 D7 lists them as load-bearing rather than optional.
/// The policy list is whatever the token maps to in the sealed file.
pub fn token_lookup(policies: &[String]) -> String {
    let mut out = String::with_capacity(256);
    out.push_str(r#"{"data":{"accessor":"","creation_time":0,"creation_ttl":0,"display_name":"kallisto","entity_id":"","expire_time":null,"explicit_max_ttl":0,"id":"","meta":null,"num_uses":0,"orphan":true,"path":"auth/token/create","policies":["#);
    for (i, policy) in policies.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        push_json_string(&mut out, policy);
    }
    out.push_str(r#"],"renewable":false,"ttl":0,"type":"service"}}"#);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn json(text: &str) -> serde_json::Value {
        serde_json::from_str(text)
            .unwrap_or_else(|e| panic!("builder produced invalid JSON: {e}\n{text}"))
    }

    #[test]
    fn a_secret_is_spliced_in_without_being_reparsed() {
        let body = json(&kv_data(
            r#"{"user":"admin","pw":"s3cr3t"}"#,
            7,
            "2026-09-17T00:00:00Z",
        ));
        assert_eq!(body["data"]["data"]["pw"], "s3cr3t");
        assert_eq!(body["data"]["metadata"]["version"], 7);
        assert_eq!(body["data"]["metadata"]["destroyed"], false);
    }

    /// Everything that is not the secret value is escaped, because keys come
    /// from a file someone else wrote.
    #[test]
    fn quotes_and_control_characters_in_keys_cannot_break_the_body() {
        let keys = vec![r#"a"b"#.to_string(), "c\nd".to_string(), "e\\f".to_string()];
        let body = json(&list_keys(&keys));
        assert_eq!(body["data"]["keys"][0], r#"a"b"#);
        assert_eq!(body["data"]["keys"][1], "c\nd");
        assert_eq!(body["data"]["keys"][2], "e\\f");
    }

    #[test]
    fn an_error_message_with_a_quote_in_it_is_still_valid_json() {
        let body = json(&errors(r#"no handler for route "we/ird""#));
        assert_eq!(body["errors"][0], r#"no handler for route "we/ird""#);
    }

    /// ADR-0016 QĐ-2: one file version, one entry, no history.
    #[test]
    fn metadata_reports_exactly_one_version() {
        let body = json(&kv_metadata(12, "2026-09-17T00:00:00Z"));
        assert_eq!(body["data"]["current_version"], 12);
        assert_eq!(body["data"]["versions"].as_object().unwrap().len(), 1);
        assert_eq!(body["data"]["versions"]["12"]["destroyed"], false);
    }

    #[test]
    fn health_carries_the_file_identity_an_operator_compares_across_machines() {
        let body = json(&health(
            false,
            1_700_000_000,
            Some(12),
            Some("\"abc\""),
            Some("2026-09-17T00:00:00Z"),
            Some(true),
        ));
        assert_eq!(body["sealed"], false);
        assert_eq!(body["kallisto_file_version"], 12);
        assert_eq!(body["kallisto_etag"], "\"abc\"");
        assert_eq!(body["version"], VAULT_COMPAT_VERSION);
        assert_eq!(body["kallisto_authorization"], "enforced");

        let empty = json(&health(true, 0, None, None, None, None));
        assert_eq!(empty["sealed"], true);
        assert!(empty["kallisto_file_version"].is_null());

        // The state an operator has to be able to spot: serving happily, with
        // no token table at all.
        let open = json(&health(false, 0, Some(1), None, Some("t"), Some(false)));
        assert_eq!(open["kallisto_authorization"], "none");
    }

    #[test]
    fn the_sys_endpoints_are_valid_json_for_any_mount_name() {
        json(&seal_status(true));
        json(&mounts("secret"));
        json(&ui_mount(r#"we"ird"#));
        json(&token_lookup(&["default".to_string(), "db".to_string()]));
    }
}
