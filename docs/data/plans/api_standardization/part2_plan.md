# Complete Vault KV v2 API Contract + E2E Test Suite

Hoàn tất tất cả deferred Data Plane endpoints từ [phase4_deferred_todo.md](file:///home/stella/workspace/naughtian-kallisto/docs/data/plans/api_standardization/phase4_deferred_todo.md) và xây dựng bộ e2e test chạy Vault CLI thật để validate tương thích.

## User Review Required

> [!IMPORTANT]
> **Scope giới hạn ở Data Plane (Port 8200) only.** Các endpoint P3 thuộc Control Plane (Port 8202) — `POST /v1/:mount/config`, `POST/PATCH/DELETE /v1/:mount/metadata/:path` — sẽ KHÔNG nằm trong plan này vì chúng cần Rust Admin Server infrastructure chưa có.

> [!WARNING]
> **`read_secret` hiện tại hardcode metadata response** (line 85 trong [http_handler.rs](file:///home/stella/workspace/naughtian-kallisto/src/server/http_handler.rs#L82-L87)): `"version":1,"created_time":"2023-01-01T00:00:00Z"`. Cần sửa để trả về metadata thật từ engine. Đây là bug hiện tại, không phải feature mới.

> [!WARNING]
> **`read_secret` không parse `?version=N` query param.** Luôn truyền `version=0` (latest) vào engine. Vault CLI sử dụng `-version=N` flag → phải sửa.

## Open Questions

> [!IMPORTANT]
> **JSON library choice cho Merge Patch:** Hiện tại handler dùng `sonic-rs` cho parsing. Hai lựa chọn:
> 1. Implement merge logic thủ công trên `sonic_rs::Value` (nhẹ, ít dependency, giữ SIMD pipeline)
> 2. Thêm `serde_json` cho merge path (đã có trong workspace deps)
>
> Khuyến nghị: Option 1 — implement merge thủ công trên `sonic_rs::Value`. Logic đơn giản (null = delete, object = recurse, scalar = replace). Giữ nguyên SIMD parsing pipeline, không cross-library conversion.

---

## Hardware Optimization Philosophy

> *Mọi dòng code trong hot path phải chiều theo phần cứng — không phải ngược lại.*

Xuyên suốt implementation, áp dụng các kỹ thuật sau:

### Branch Prediction

```rust
// ❌ Sai: error path đầu tiên, CPU predict sai trên happy case
if let Err(e) = operation() {
    return Err(e);
}

// ✅ Đúng: happy path thẳng, error path đánh dấu #[cold]
let result = operation();
if result.is_ok() {
    // hot path tiếp tục linear — CPU prefetch đúng
    process(result.unwrap());
} else {
    handle_error(result.unwrap_err()); // #[cold] #[inline(never)]
}
```

- `#[cold]` + `#[inline(never)]` trên mọi error formatting function — đẩy error code ra khỏi instruction cache
- Happy path luôn là nhánh "fall-through" (không jump) — CPU branch predictor predict forward-not-taken
- Sắp xếp `match` arms: case phổ biến nhất đặt đầu tiên
- `if let` chains: điều kiện likely-true đặt trước

### Cache-Line Aware Layout

```rust
// KeyMetadata hot fields (read trên mọi request) pack vào 1 cache line (64 bytes)
// current_version (4B) + max_versions (4B) + cas_required (1B) + delete_version_after_ms (8B) = 17B
// → fit trong 1 cache line, CPU load 1 lần là đủ cho read_version metadata check

// Cold fields (custom_metadata, versions Vec) → truy cập riêng, không ô nhiễm L1
```

- Struct field ordering: hot fields trước, cold fields sau
- `VersionState` giữ 17 bytes — 3 entries fit trong 1 cache line (64B)
- Avoid false sharing: mỗi field được truy cập bởi cùng 1 operation nằm cạnh nhau

### Zero-Allocation Response Construction

```rust
// ❌ Sai: format!() allocates, serde_json::to_string allocates, intermediate String allocates
let json = serde_json::to_string(&response)?;

// ✅ Đúng: pre-sized buffer, push_str trực tiếp, zero intermediate allocation
let mut buf = String::with_capacity(256 + payload_len);
buf.push_str(r#"{"data":{"data":"#);
buf.push_str(&payload.value); // value đã là JSON string
buf.push_str(r#","metadata":{"version":"#);
// itoa crate cho integer → string mà không allocate
```

- `String::with_capacity()` exact-size trên mọi response builder — zero realloc
- `itoa` crate cho integer formatting (no allocation, write trực tiếp vào buffer)
- Boolean → `"true"`/`"false"` literal push, không format
- Reuse `sonic_rs` zero-copy parsing — không deserialize-rồi-serialize lại

### SIMD-Accelerated JSON (sonic-rs)

- Giữ `sonic-rs` cho tất cả JSON parsing trên hot path
- `sonic_rs::get()` lazy evaluation — chỉ parse field cần, skip phần còn lại
- Merge Patch: dùng `sonic_rs::Value` tree manipulation trực tiếp, không convert sang `serde_json::Value`
- Subkeys endpoint: dùng `sonic_rs` traverse + null-replacement in-place

### Inline Hints

```rust
#[inline]                    // Small, hot — inline vào caller, tránh call overhead
fn build_meta_key(path: &str) -> String { ... }

#[inline(never)] #[cold]    // Error path — đẩy ra khỏi instruction cache
fn format_engine_error(e: EngineError) -> (StatusCode, String) { ... }
```

- `#[inline]` trên: `build_meta_key`, `build_version_key`, `extract_mount_and_path`, `parse_versions_list`, query param helpers
- `#[inline(never)]` + `#[cold]` trên: tất cả error formatting trong `AppError::into_response`, `format!` calls trong error paths

### Prefetch & Zero-Copy rkyv

- `read_version` đã dùng zero-copy rkyv (`archived_root`) — giữ nguyên pattern này
- Subkeys endpoint: zero-copy read value bytes → parse JSON → strip leaves → respond. Không deserialize `SecretPayload` đầy đủ nếu chỉ cần value string.
- List keys: `BTreeIndex::get_all_paths()` trả `Vec<String>` — cân nhắc trả iterator để tránh collect toàn bộ khi chỉ cần prefix filter

### Duration Formatting (No Dependencies)

```rust
// ❌ Sai: chrono::Duration → format string (pull 50KB dependency)
// ✅ Đúng: arithmetic division chain, write trực tiếp
#[inline]
fn ms_to_vault_duration(ms: u64) -> String {
    let secs = ms / 1000;
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    // itoa + push_str, no format! macro overhead
}
```

---

## Proposed Changes

### Phase 1: Foundation — Query Params + Fix `read_secret` Response

Sửa các bug hiện tại trước khi thêm endpoint mới.

---

#### [MODIFY] [traits.rs](file:///home/stella/workspace/naughtian-kallisto/src/engine/traits.rs)
- Thêm `custom_metadata: HashMap<String, String>` vào `KeyMetadata` (direct add, không cần migration)
- Thêm `#[serde(default)]` để `Default` trait auto-fills khi deserialize old data

#### [MODIFY] [http_handler.rs](file:///home/stella/workspace/naughtian-kallisto/src/server/http_handler.rs)

**Query param parsing (zero-alloc):**
```rust
/// Extract ?version=N from URI query string without allocating a HashMap
#[inline]
fn extract_version_param(uri: &Uri) -> u32 {
    // Manual byte scan — faster than url::form_urlencoded iterator
    uri.query()
        .and_then(|q| {
            // Scan for "version=" prefix, then parse digits inline
            q.find("version=")
                .map(|i| &q[i + 8..])
                .and_then(|s| {
                    let end = s.find('&').unwrap_or(s.len());
                    s[..end].parse::<u32>().ok()
                })
        })
        .unwrap_or(0)
}
```

**Response builder sửa lại:**
- Đọc `VersionState` thật từ metadata (created_time, deletion_time, destroyed)
- Format timestamps ISO 8601 bằng arithmetic, không dùng chrono
- `#[cold] #[inline(never)]` trên mọi error branch

#### [MODIFY] [kv_engine.rs](file:///home/stella/workspace/naughtian-kallisto/src/engine/kv_engine.rs)
- `read_version` trả thêm `VersionState` metadata cùng payload (single cache lookup, không 2 lần)
- Signature change: `read_version` return `(SecretPayload, VersionState)` hoặc thêm method mới `read_version_with_meta`

---

### Phase 2: New Endpoints — PATCH, Subkeys, LIST

#### [MODIFY] [http_handler.rs](file:///home/stella/workspace/naughtian-kallisto/src/server/http_handler.rs)

**2a. `PATCH /v1/:mount/data/:path` — JSON Merge Patch (RFC 7396)**

```rust
/// RFC 7396 JSON Merge Patch — in-place on sonic_rs::Value
/// Branch-prediction friendly: most values are scalars (fast path),
/// objects (recurse) and nulls (delete) are rarer paths.
fn json_merge_patch(target: &mut Value, patch: &Value) {
    if !patch.is_object() {
        *target = patch.clone();
        return; // Fast scalar replacement — most common in practice
    }
    // Object merge — less common, recursive
    if !target.is_object() {
        *target = Value::default(); // coerce to empty object
    }
    for (key, value) in patch.as_object().unwrap().iter() {
        if value.is_null() {
            target.as_object_mut().unwrap().remove(key);
        } else {
            let entry = target.as_object_mut().unwrap()
                .entry(key).or_insert(Value::default());
            json_merge_patch(entry, value);
        }
    }
}
```

- Check `Content-Type: application/merge-patch+json` header
- Read current → merge → write as new version
- `#[inline]` trên merge function (recursive nhưng depth thường ≤ 3)

**2b. `GET /v1/:mount/subkeys/:path` — Read Secret Subkeys**

```rust
/// Strip leaf values to null, respecting depth limit
/// Breadth-first traversal for better cache locality than depth-first
fn strip_to_subkeys(value: &mut Value, current_depth: u32, max_depth: u32) {
    if max_depth > 0 && current_depth >= max_depth {
        *value = Value::Null; // At depth limit, treat as leaf
        return;
    }
    if let Some(obj) = value.as_object_mut() {
        for (_key, val) in obj.iter_mut() {
            if val.is_object() {
                strip_to_subkeys(val, current_depth + 1, max_depth);
            } else {
                *val = Value::Null; // Leaf → null (no allocation)
            }
        }
    } else {
        *value = Value::Null;
    }
}
```

- Parse `?depth=N` param (same zero-alloc pattern as version param)
- Zero-copy read from rkyv → parse JSON → strip → respond

**2c. `GET /v1/:mount/metadata/:path?list=true` — List Keys**

- Check `?list=true` query param in existing `read_metadata` handler
- Branch early: `list=true` → fast path calling `engine.list_keys()`, skip metadata read entirely
- Response: pre-sized buffer `{"data":{"keys":[` + join keys + `]}}`

Router update:
```rust
pub fn vault_kv_router(state: AppState) -> Router {
    Router::new()
        .route("/v1/:mount/data/*path", get(read_secret).post(write_secret).delete(delete_latest).patch(patch_secret))
        .route("/v1/:mount/subkeys/*path", get(read_subkeys))
        .route("/v1/:mount/delete/*path", post(soft_delete_versions))
        .route("/v1/:mount/undelete/*path", post(undelete_versions))
        .route("/v1/:mount/destroy/*path", put(destroy_versions))
        .route("/v1/:mount/metadata/*path", get(read_metadata))
        .nest("/v1/sys", sys_handler::router::<AppState>())
        .with_state(state)
}
```

---

### Phase 3: ISO 8601 Duration + Time Formatting

#### [NEW] [src/server/time_format.rs](file:///home/stella/workspace/naughtian-kallisto/src/server/time_format.rs)

Zero-dependency, hardware-friendly time formatting:

```rust
/// Convert epoch milliseconds to RFC 3339 string.
/// Pure arithmetic — no chrono, no alloc beyond the output String.
/// Division chain optimized: large units first → small units,
/// CPU branch predictor sees sequential subtract-divide pattern.
#[inline]
pub fn epoch_ms_to_rfc3339(ms: u64) -> String {
    let total_secs = ms / 1000;
    // ... civil time calculation via days-since-epoch arithmetic
    // Write directly into pre-sized String::with_capacity(30)
}

/// Parse Go-style duration "3h25m19s" → milliseconds.
/// Single-pass byte scanner — no regex, no split, no allocations.
/// Each byte triggers at most 1 branch (digit vs letter).
#[inline]
pub fn parse_vault_duration(s: &str) -> Result<u64, &'static str> {
    let mut ms: u64 = 0;
    let mut current_num: u64 = 0;
    for b in s.as_bytes() {
        match b {
            b'0'..=b'9' => current_num = current_num * 10 + (b - b'0') as u64,
            b'h' => { ms += current_num * 3_600_000; current_num = 0; }
            b'm' => { ms += current_num * 60_000; current_num = 0; }
            b's' => { ms += current_num * 1_000; current_num = 0; }
            _ => return Err("invalid duration character"),
        }
    }
    Ok(ms)
}

/// Format milliseconds → Go-style duration "3h25m19s"
/// itoa-style digit emission, no format! macro.
#[inline]
pub fn ms_to_vault_duration(ms: u64) -> String {
    // Pre-sized: max "99999h59m59s" = 12 chars
    let mut buf = String::with_capacity(12);
    let secs = ms / 1000;
    // ... push digits directly
}
```

#### [MODIFY] [src/server/mod.rs](file:///home/stella/workspace/naughtian-kallisto/src/server/mod.rs)
- Thêm `pub mod time_format;`

---

### Phase 4: E2E Test Suite — Vault CLI Compatibility

#### [NEW] [tests/e2e_vault_compat.rs](file:///home/stella/workspace/naughtian-kallisto/tests/e2e_vault_compat.rs)

```rust
/// E2E test: spawn Kallisto → run Vault CLI commands → validate output
/// Requires `vault` binary in PATH (install via vault_version in CI)
///
/// Usage:
///   cargo test --test e2e_vault_compat -- --ignored
///   (ignored by default, only run explicitly for e2e)
```

Test matrix:

| # | Vault CLI Command | Validates |
|---|---|---|
| 1 | `vault kv put secret/app/db user=admin pass=s3cr3t` | Write secret (POST data) |
| 2 | `vault kv get secret/app/db` | Read latest version |
| 3 | `vault kv get -version=1 secret/app/db` | `?version=N` query param |
| 4 | `vault kv patch secret/app/db pass=new_pass` | PATCH merge |
| 5 | `vault kv get -field=pass secret/app/db` | Merged field value |
| 6 | `vault kv metadata get secret/app/db` | Version history + custom_metadata |
| 7 | `vault kv list secret/app/` | LIST keys (?list=true) |
| 8 | `vault kv delete -versions=1 secret/app/db` | Soft-delete specific version |
| 9 | `vault kv undelete -versions=1 secret/app/db` | Undelete |
| 10 | `vault kv destroy -versions=1 secret/app/db` | Permanent destroy |

#### [NEW] [tests/e2e/docker-compose.test.yml](file:///home/stella/workspace/naughtian-kallisto/tests/e2e/docker-compose.test.yml)

```yaml
services:
  kallisto:
    build: { context: ../.. }
    ports: ["18200:8200"]
    environment:
      - WORKERS=1
      - DB_PATH=/tmp/e2e_test

  vault-cli:
    image: hashicorp/vault:latest
    entrypoint: ["sh", "-c", "sleep infinity"]
    environment:
      - VAULT_ADDR=http://kallisto:8200
      - VAULT_TOKEN=root
    depends_on: [kallisto]
```

---

### Phase 5: Update Docs + Roadmap Checklist

#### [MODIFY] [phase4_deferred_todo.md](file:///home/stella/workspace/naughtian-kallisto/docs/data/plans/api_standardization/phase4_deferred_todo.md)
- Mark completed items

#### [MODIFY] [roadmap.md](file:///home/stella/workspace/naughtian-kallisto/docs/data/plans/roadmap.md)
- Check off completed P1 deferred items

---

## File Impact Summary

| File | Action | Est. Lines |
|---|---|---|
| [traits.rs](file:///home/stella/workspace/naughtian-kallisto/src/engine/traits.rs) | MODIFY | +5 |
| [kv_engine.rs](file:///home/stella/workspace/naughtian-kallisto/src/engine/kv_engine.rs) | MODIFY | +20 |
| [http_handler.rs](file:///home/stella/workspace/naughtian-kallisto/src/server/http_handler.rs) | MODIFY | +180 |
| [time_format.rs](file:///home/stella/workspace/naughtian-kallisto/src/server/time_format.rs) | NEW | ~80 |
| [mod.rs](file:///home/stella/workspace/naughtian-kallisto/src/server/mod.rs) | MODIFY | +1 |
| [e2e_vault_compat.rs](file:///home/stella/workspace/naughtian-kallisto/tests/e2e_vault_compat.rs) | NEW | ~200 |
| [docker-compose.test.yml](file:///home/stella/workspace/naughtian-kallisto/tests/e2e/docker-compose.test.yml) | NEW | ~20 |
| docs | MODIFY | checkbox updates |

---

## Verification Plan

### Automated Tests
```bash
# Unit tests (existing + new handler tests)
make test

# E2E with Vault CLI (requires docker + vault binary)
make e2e
```

### Manual Verification
- `curl` từng endpoint mới, verify JSON response format
- `vault kv` CLI commands trỏ tới Kallisto, so sánh output với Vault thật
- Benchmark trước/sau: `wrk` hoặc `k6` trên `read_secret` để confirm zero regression từ query param parsing
