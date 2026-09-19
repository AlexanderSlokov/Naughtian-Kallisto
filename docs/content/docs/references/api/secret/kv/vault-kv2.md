---
title: "HTTP API (Vault KV-v2 Compatible)"
weight: 2
---

Kallisto speaks a **read-only** subset of Vault's KV-v2 HTTP API on port **8200**, bound to
loopback. Routing is `/v1/:mount/:action/:path`, where the mount defaults to `secret`.

Every body on this page was captured from a running resolver, not written from memory. An earlier
version of this document described writing, soft-delete and destroy long after ADR-0015 D1 turned
all three into `403` — which is the failure mode a reference page is most prone to, and the reason
each example below is meant to be re-run rather than trusted.

> **Everything that writes answers `403 permission denied`.** That is the product, not a missing
> feature: a resolver that cannot write is a resolver whose stolen credentials are worth nothing.
> See [What is refused](#what-is-refused).

## Authentication

A token goes in either header; the SDKs are not unanimous, so both are accepted.

```
X-Vault-Token: s.<token>
Authorization: Bearer s.<token>
```

Tokens live as keyed hashes **inside the sealed file** (ADR-0015 D8). Mint one with
`kallisto-ctl mint-token`; revoking means deleting its row and re-sealing. There is no token API.

A file carrying no token table permits every read — the single-app sidecar case. `sys/health`
reports which mode you are in as `kallisto_authorization`.

## Reading a secret

```bash
curl -H "X-Vault-Token: $VAULT_TOKEN" \
  http://127.0.0.1:8200/v1/secret/data/myapp/db
```

```json
{
  "request_id": "", "lease_id": "", "renewable": false, "lease_duration": 0,
  "data": {
    "data": { "password": "duck-fixture-not-a-credential", "username": "admin" },
    "metadata": {
      "created_time": "2026-09-19T10:35:38.540Z",
      "custom_metadata": null,
      "deletion_time": "",
      "destroyed": false,
      "version": 7
    }
  },
  "wrap_info": null, "warnings": null, "auth": null
}
```

`metadata.version` is the **content version of the whole file**, not a per-secret counter
(ADR-0016 QĐ-2). Every secret in one file reports the same number, and it rises each time an
operator seals a new file.

### `?version=N`

```bash
curl -H "X-Vault-Token: $VAULT_TOKEN" \
  'http://127.0.0.1:8200/v1/secret/data/myapp/db?version=7'
```

Asking for the version currently served returns it. **Any other version is `404`** with Vault's
empty error list, because this process holds exactly one:

```json
{"errors":[]}
```

Per-secret history is not kept. Older values live in your git history and your bucket's own
versioning; the only counter Kallisto keeps is the file's, and it refuses any file older than the
one it already holds.

## Metadata

```bash
curl -H "X-Vault-Token: $VAULT_TOKEN" \
  http://127.0.0.1:8200/v1/secret/metadata/myapp/db
```

```json
{
  "data": {
    "cas_required": false,
    "created_time": "2026-09-19T10:35:38.540Z",
    "current_version": 7,
    "custom_metadata": null,
    "delete_version_after": "0s",
    "max_versions": 0,
    "oldest_version": 7,
    "updated_time": "2026-09-19T10:35:38.540Z",
    "versions": { "7": { "created_time": "2026-09-19T10:35:38.540Z", "deletion_time": "", "destroyed": false } }
  }
}
```

Exactly one entry in `versions`. Reading metadata opens nothing — no decryption happens.

## Listing

Vault accepts the made-up `LIST` verb *and* `GET ?list=true`, and the SDKs are split: the Go client
sends `LIST`, several others send the query parameter. **Both work here**, and supporting only one
is the easiest way to pass every unit test and still fail `vault kv list`.

```bash
curl -X LIST -H "X-Vault-Token: $VAULT_TOKEN" \
  http://127.0.0.1:8200/v1/secret/metadata/myapp
curl -H "X-Vault-Token: $VAULT_TOKEN" \
  'http://127.0.0.1:8200/v1/secret/metadata/myapp?list=true'
```

```json
{"data":{"keys":["db","sub/","web"]}}
```

Immediate children only. A nested path contributes its next segment with a trailing slash, once,
however many secrets sit beneath it. An empty prefix is `404`, as in Vault.

Listing needs the `list` capability on the **`metadata`** path, while reading needs `read` on the
**`data`** path — KV-v2's own quirk, kept deliberately (ADR-0015 D8):

```json
{ "app": [
    {"path": "secret/data/myapp/*",     "capabilities": ["read"]},
    {"path": "secret/metadata/myapp/*", "capabilities": ["list", "read"]}
]}
```

## System endpoints

### `GET /v1/sys/health`

```json
{
  "initialized": true, "sealed": false, "standby": false, "performance_standby": false,
  "replication_performance_mode": "disabled", "replication_dr_mode": "disabled",
  "server_time_utc": 1789814141, "version": "1.13.0",
  "cluster_name": "kallisto", "cluster_id": "",
  "kallisto_version": "1.0.0",
  "kallisto_file_version": 7,
  "kallisto_etag": null,
  "kallisto_loaded_at": "2026-09-19T10:35:38.540Z",
  "kallisto_authorization": "enforced"
}
```

`200` when a file is loaded, **`503` when none is** — the same two answers Vault gives for unsealed
and sealed, so an existing liveness probe keeps working without being told about any of this.

The `kallisto_*` fields are additions (ADR-0015 D4). An SDK ignores them; a human asking three
machines whether they agree needs them. `kallisto_authorization` is `"enforced"`, `"none"` or
`null`, and `"none"` means the file has no token table so every read is permitted.

### `GET /v1/sys/init`

```json
{"initialized":true}
```

Always initialised: there is no unseal ceremony to be part-way through. It exists because SDKs ask
for it before anything else — `hvac`'s `is_initialized()` is exactly this call, and a `404` here
stops a client on its first line.

### `GET /v1/sys/seal-status`

```json
{"type":"shamir","initialized":true,"sealed":false,"t":1,"n":1,"progress":0,"nonce":"",
 "version":"1.13.0","migration":false,"cluster_name":"kallisto","cluster_id":"",
 "recovery_seal":false,"storage_type":"kallisto"}
```

`200` either way; `sealed` carries the truth. The shamir fields are there because clients branch on
them — there are no key shards.

### `GET /v1/sys/mounts`

```json
{"data":{"secret/":{"accessor":"kv_kallisto",
 "config":{"default_lease_ttl":0,"force_no_cache":false,"max_lease_ttl":0},
 "description":"key/value secret storage","local":false,
 "options":{"version":"2"},"seal_wrap":false,"type":"kv","uuid":"kallisto"}}}
```

One mount, KV version 2, because that is all there is.

### `GET|POST /v1/auth/token/{lookup-self,renew-self}`

```json
{"data":{"accessor":"","creation_time":0,"creation_ttl":0,"display_name":"kallisto",
 "entity_id":"","expire_time":null,"explicit_max_ttl":0,"id":"","meta":null,
 "num_uses":0,"orphan":true,"path":"auth/token/create",
 "policies":["app"],"renewable":false,"ttl":0,"type":"service"}}
```

Both answer the same thing: nothing expires here, so `renew-self` renews nothing and says so with a
zero TTL. A file with no token table reports `["root"]`, Vault's word for a token with no
restrictions.

### `GET /v1/sys/metrics`

Prometheus text exposition, unauthenticated, loopback-only like everything else.

```
# HELP kallisto_access_log_dropped_total Access log lines discarded because the queue was full.
# TYPE kallisto_access_log_dropped_total counter
kallisto_access_log_dropped_total 0
# HELP kallisto_requests_total Requests answered, by outcome class.
# TYPE kallisto_requests_total counter
kallisto_requests_total{outcome="ok"} 10
kallisto_requests_total{outcome="denied"} 4
kallisto_requests_total{outcome="not_found"} 2
```

Also `kallisto_sealed`, `kallisto_file_version`, `kallisto_secrets`, `kallisto_tokens`,
`kallisto_authorization_enforced` and `kallisto_refresh_failures_total`.

**Alert on `kallisto_access_log_dropped_total`.** A non-zero value means the process is shedding its
own bookkeeping to keep serving secrets — the right trade, and a sign that something is driving
serious load at a daemon that handles tens of thousands of reads a second without noticing.

## What is refused

| Route | Method |
| --- | --- |
| `/v1/:mount/data/*` | `PUT` `POST` `PATCH` `DELETE` |
| `/v1/:mount/delete/*`, `undelete/*`, `destroy/*`, `subkeys/*` | any |

All answer `403` with Vault's own wording — clients match on this string, so it is compatibility
surface rather than a message we are free to improve:

```json
{"errors":["permission denied"]}
```

These routes exist so they can answer `403` rather than `404`. A `404` would read as "not deployed
yet"; `403` says the door exists and is shut.

## Status codes

| Code | When | Body |
| --- | --- | --- |
| `200` | read succeeded | as above |
| `403` | no token, unknown token, path outside policy, **or any write** | `{"errors":["permission denied"]}` |
| `404` | path is inside your policy but absent, or `?version=` is not the one held | `{"errors":[]}` |
| `429` | over the per-worker rate limit; carries `Retry-After` | `{"errors":["request rate exceeded"]}` |
| `503` | no file loaded yet — Vault's sealed state (D14) | `{"errors":["Vault is sealed"]}` |

**A refusal is not an existence oracle.** Authorization is checked *before* the lookup, so a path
you may not read answers `403` whether or not it exists. Only paths inside your policy can produce
a `404`. Real Vault answers `400 missing client token` when no token is sent; Kallisto answers `403`
for that too, because the difference tells a caller something a `403` already told them.

## Not implemented

No auth methods, dynamic secrets, leases, PKI, transit, namespaces, replication, or `sys/*`
beyond the handful above. Kallisto reads one file. If you need any of that, run OpenBao — and put
Kallisto in front of it if you like.
