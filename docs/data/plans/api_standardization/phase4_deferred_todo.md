# Kallisto — Phase 4 Deferred TODO

> [!IMPORTANT]
> These items were intentionally excluded from the Phase 4 API standardization implementation
> completed on 2026-05-23. The core KV-v2 CRUD + versioning endpoints are done. These are
> the remaining features needed for full Vault KV-v2 API compatibility.

---

## Completed in Phase 4 ✅

| Endpoint | Method | Status |
|----------|--------|--------|
| `/v1/:mount/data/:path` | GET | ✅ Vault envelope + `?version=N` |
| `/v1/:mount/data/:path` | POST | ✅ Parse `data` + `options.cas` |
| `/v1/:mount/data/:path` | DELETE | ✅ Soft-delete latest version |
| `/v1/:mount/delete/:path` | POST | ✅ Soft-delete specific versions |
| `/v1/:mount/undelete/:path` | POST | ✅ Restore soft-deleted versions |
| `/v1/:mount/destroy/:path` | PUT | ✅ Permanently destroy versions |
| `/v1/:mount/metadata/:path` | GET | ✅ Read key metadata + version history |
| `/v1/sys/health` | GET | ✅ Mock |
| `/v1/sys/mounts` | GET | ✅ Mock |
| `/v1/sys/seal-status` | GET | ✅ Mock |
| Dynamic mount routing | — | ✅ Via EngineRegistry |
| `undelete` on ISecretEngine | — | ✅ Added to interface + KvEngine |
| `list_keys` on ISecretEngine | — | ✅ Added to interface + KvEngine |

---

## Deferred Work Items

### P1 — High Priority

#### 1. `PATCH /v1/:mount/data/:path` — JSON Merge Patch ✅ (Implemented in Rust `http_handler.rs`)
- **Vault Spec:** [RFC 7396](https://datatracker.ietf.org/doc/html/draft-ietf-appsawg-json-merge-patch-07)
- **Requires:** `Content-Type: application/merge-patch+json` header check
- **Behavior:** Read current version's data, merge with patch payload, create new version
- **Implementation Notes:**
  - Implemented manually on `sonic_rs::Value` for SIMD zero-alloc performance.

#### 2. `GET /v1/:mount/subkeys/:path` — Read Secret Subkeys ✅ (Implemented in Rust `http_handler.rs`)
- **Vault Spec:** Returns key structure with values replaced by `null`
- **Query params:** `?version=N`, `?depth=N`
- **Behavior:** Strip leaf values, preserve key hierarchy, respect depth limit
- **Implementation Notes:**
  - Implemented via `strip_to_subkeys` on `sonic_rs::Value`.

#### 3. `LIST /v1/:mount/metadata/:path` — List Keys (HTTP LIST method) ✅ (Implemented in Rust `http_handler.rs`)
- **Status:** Handler exists but LIST is non-standard HTTP method
- **Current issue:** Most HTTP clients send `GET` with `?list=true` instead
- **TODO:** Add support for `GET /v1/:mount/metadata/:path?list=true` as alternative
- **Implementation Notes:**
  - `read_metadata` now intercepts `?list=true` and calls `BTreeIndex` via `list_keys()`.

### P2 — Medium Priority

#### 4. `custom_metadata` Field Support ✅ (Implemented in Rust `traits.rs`)
- **Vault Spec:** `map<string, string>` user-provided metadata per key
- **Implementation Notes:**
  - Added `custom_metadata: HashMap<String, String>` to `KeyMetadata` with `#[serde(default)]`.

#### 5. `?depth=N` Query Parameter for Subkeys ✅ (Implemented in Rust `http_handler.rs`)
- **Depends on:** Item #2 (subkeys endpoint)
- **Implementation Notes:** Zero-alloc `extract_depth_param` added.

#### 6. ISO 8601 Duration Format for `delete_version_after` ✅ (Implemented in Rust `time_format.rs`)
- **Vault uses:** `"3h25m19s"` format
- **Current:** Stored as `uint64_t` milliseconds, displayed as `"Nms"`
- **Implementation Notes:** Implemented zero-dependency `time_format.rs` arithmetic parsers.

### P3 — Low Priority (Admin/Control Plane — Port 8202)

#### 7. `POST /v1/:mount/config` — Configure Engine (Port 8202)
- **Belongs to:** Rust Control Plane
- **Behavior:** Set `max_versions`, `cas_required`, `delete_version_after` on engine mount
- **Requires:** New FFI function to set engine config from Rust

#### 8. `POST /v1/:mount/metadata/:path` — Create/Update Key Metadata (Port 8202)
- **Belongs to:** Rust Control Plane
- **Behavior:** Set per-key `max_versions`, `cas_required`, `custom_metadata`

#### 9. `PATCH /v1/:mount/metadata/:path` — Patch Key Metadata (Port 8202)
- **Belongs to:** Rust Control Plane
- **Requires:** JSON Merge Patch (same as Item #1)

#### 10. `DELETE /v1/:mount/metadata/:path` — Delete All Versions + Metadata (Port 8202)
- **Belongs to:** Rust Control Plane (destructive admin operation)
- **Requires:** New FFI function: `delete_all_versions(path)` on KvEngine

---

## Technical Notes for Next Agent

> [!NOTE]
> **JSON Parsing Strategy:** The current implementation uses manual string parsing for JSON.
> `simdjson` is linked to `kallisto_server_lib` (compile flag `KALLISTO_HAS_SIMDJSON`).
> `nlohmann_json` is also available via vcpkg. For Items #1 and #2, consider using one
> of these libraries for proper JSON object manipulation rather than string surgery.

> [!NOTE]
> **Engine Interface Contract:** Any new method added to `ISecretEngine` MUST also be added to:
> 1. `engine_concept.hpp` — C++20 concept `ValidEngine`
> 2. `kv_engine.hpp` / `kv_engine.cpp` — Concrete implementation
> 3. `test_engine_registry.cpp` — MockEngine (MOCK_METHOD)

> [!NOTE]
> **Test Infrastructure:** `test_http_handler.cpp` provides `seedSecret()` helper that
> directly calls the V2 engine API. Use this pattern for seeding test data instead of
> going through the HTTP POST handler.
