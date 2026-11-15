---
title: Metrics and logs
description: Every metric on /v1/sys/metrics, and the format of the access log.
sidebar:
  order: 5
---

## Metrics

`GET /v1/sys/metrics` returns Prometheus text exposition format 0.0.4. Every
metric carries `HELP` and `TYPE`. Counters belong to one worker each and are
summed at scrape time, so any worker can answer a scrape.

| Metric | Type | Meaning |
|---|---|---|
| `kallisto_access_log_dropped_total` | counter | Access log lines discarded because the queue was full. Present at zero. |
| `kallisto_requests_total{outcome}` | counter | Requests answered, by outcome class. |
| `kallisto_sealed` | gauge | 1 when no file has loaded and every read answers 503. |
| `kallisto_file_version` | gauge | Content version being served, 0 when sealed. |
| `kallisto_secrets` | gauge | Secrets in the file being served. |
| `kallisto_tokens` | gauge | Tokens in the file being served. |
| `kallisto_authorization_enforced` | gauge | 0 when the file carries no token table. |
| `kallisto_refresh_failures_total` | counter | Polls that did not yield a servable file. |

### `outcome` labels

| Label | Status |
|---|---|
| `ok` | 2xx |
| `denied` | 403 |
| `not_found` | 404 |
| `rate_limited` | 429 |
| `sealed` | 503 |
| `error` | anything else |

### Notes

- `kallisto_refresh_failures_total` counts rejected files, failed
  authentication, rollback refusals and unreachable sources, on every failed
  poll. It reads **1 after a first boot**, because a fresh machine has no
  fallback copy to warm from.
- With `log.enabled: false`, nothing is enqueued, so
  `kallisto_access_log_dropped_total` stays at zero.

See [monitor Kallisto](/kallisto/how-to/monitor-kallisto/) for the alerts
built on these.

## Access log

One line per request on **stdout**. stdout carries nothing else.

```
ts=<millis> seq=<n> worker=<n> method=<M> action=<A> path=<id> token=<id> status=<code> authz=on|off
```

| Field | Meaning |
|---|---|
| `ts` | Unix time in milliseconds when the request was logged. |
| `seq` | Sequence number. Lines from several workers can reach the writer out of order. |
| `worker` | The worker that served the request. |
| `method` | HTTP method. |
| `action` | One of `data`, `metadata`, `list`, `sys`, or `-`. Never taken from the URI. |
| `path` | 16 hex characters of a keyed HMAC-SHA256 of the path. |
| `token` | The same for the presented token, or `-` for none. |
| `status` | HTTP status code. |
| `authz` | `on` when the file enforces authorization. |

Lines are fixed at 192 bytes and allocate nothing.

### Identifiers

Paths and tokens are keyed hashes because a deployment has a few dozen
guessable paths, and a bare digest over that set is reversed by computing the
same digests. When the file has a `token_key`, the log key is derived from it
under its own label, so identifiers stay stable across restarts and an operator
holding the file can match a line to a path. With no token table, the process
generates a random key at startup and identifiers correlate only within one
process lifetime. The server says which case applies on stderr.

## Error log

**stderr** carries operational messages: the startup banner, each refresh
outcome, hardening that could not be applied. It follows the same rules as the
access log. No secret value appears, and paths appear only as identifiers.

## Not an audit log

The access log records after serving and drops lines under load. It is
unsuitable as evidence of who read what. A test in the Kallisto repository
keeps anything in the code from being named an audit log.
