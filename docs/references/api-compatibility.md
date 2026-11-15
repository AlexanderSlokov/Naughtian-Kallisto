---
title: Vault API compatibility
description: Which Vault HTTP routes Kallisto answers, how each one behaves, and where it differs from Vault.
sidebar:
  order: 4
---

Kallisto implements the read half of the Vault KV-v2 API, plus the `sys` and
`auth/token` routes an unmodified Vault SDK calls before its first real
request. Compatibility is tested against three real clients: the official Go
client (`hashicorp/vault/api`), Python's `hvac`, and PHP's `csharpru/vault-php`.

All routes are under `/v1/`. In the tables, `secret` stands for the configured
`mount`.

## KV-v2

| Request | Behaviour |
|---|---|
| `GET /v1/secret/data/<path>` | The secret. `metadata.version` is the file's content version. |
| `GET /v1/secret/data/<path>?version=N` | The secret if `N` is the current content version, otherwise 404. |
| `GET /v1/secret/metadata/<path>` | Metadata, with one entry in `versions`. |
| `LIST /v1/secret/metadata/<path>` | Keys under the path. |
| `GET /v1/secret/metadata/<path>?list=true` | The same listing, spelled as a GET. |
| `PUT`, `POST`, `PATCH`, `DELETE` on `data/` or `metadata/` | **403** `permission denied` |
| Any method on `subkeys/`, `delete/`, `undelete/`, `destroy/` | **403** `permission denied` |

The write-side routes are declared on purpose so they answer 403. A 404 would
read as "not deployed yet". A 403 says the door exists and is shut.

A request to a mount other than the configured one answers 404.

### No version history

The file holds current values only. `?version=N` answers for the current
version and 404 for any other. Older values live in your git history and
bucket versioning. `LIST` treats `app` and `app/` as the same directory, as
Vault does.

## System and token routes

| Request | Behaviour |
|---|---|
| `GET /v1/sys/health` | 200 when a file is loaded, 503 when sealed. Adds Kallisto fields, below. |
| `GET /v1/sys/seal-status` | Real sealed state. |
| `GET /v1/sys/init` | Always initialised. |
| `GET /v1/sys/mounts` | The configured KV-v2 mount. |
| `GET /v1/sys/internal/ui/mounts/<path>` | Mount lookup, as SDKs use to detect KV version. |
| `GET /v1/sys/metrics` | Prometheus text format. See [metrics and logs](/kallisto/references/metrics/). |
| `/v1/auth/token/lookup-self`, `/v1/auth/token/renew-self` | Answered so SDK startup succeeds. |

None of these serve a secret.

### Extra fields on `sys/health`

| Field | Meaning |
|---|---|
| `kallisto_file_version` | Content version being served, `null` when sealed. |
| `kallisto_etag` | ETag of the object last loaded from a bucket. |
| `kallisto_loaded_at` | When the current file was loaded, RFC 3339. |
| `kallisto_authorization` | `enforced` with a token table, `none` without. |
| `kallisto_version` | Version string of the running binary. |

## Tokens

A token is read from `X-Vault-Token`, or from `Authorization: Bearer <token>`.
With no token table in the file, tokens are ignored and every read is
permitted. With one, every request needs a token whose policies allow the path,
and the check runs before the lookup, so a 403 reveals nothing about whether a
path exists.

## Status codes

| Code | When |
|---|---|
| 200 | Served. |
| 403 | A write, or a read the token's policies do not allow. |
| 404 | No such path, version or mount. |
| 429 | Rate limit exceeded. Carries `Retry-After`, never zero. |
| 503 | Sealed: no file has loaded yet. |

## What Vault has and Kallisto does not

Auth methods, dynamic secrets, leases, token expiry and revocation endpoints,
PKI, transit, other secret engines, and any write. If you need these, run
OpenBao or Vault. Kallisto can sit beside it for the operational secrets your
services read on every request.
