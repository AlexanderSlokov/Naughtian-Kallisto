---
title: Configure the source
description: Point Kallisto at a sealed file on an S3-compatible bucket or on local disk, and set the refresh interval and fallback copy.
sidebar:
  order: 2
---

Kallisto reads one sealed file. `spec.source` says where it comes from. It
polls a file somebody else wrote and stores nothing of its own apart from an
encrypted fallback copy.

## From an S3-compatible bucket

```yaml
apiVersion: kallisto/v1
kind: Resolver
spec:
  source:
    type: bucket
    endpoint: https://s3.example.com
    bucket: kallisto
    objectKey: prod/payment.kal
    region: auto
    pathStyle: true
  cacheDir: /var/lib/kallisto
```

Set the credentials in the environment. They have no field in the file:

```bash
export KALLISTO_S3_ACCESS_KEY_ID=...
export KALLISTO_S3_SECRET_ACCESS_KEY=...
```

Requests are SigV4-signed with a 15-second timeout. Each poll sends the ETag it
last saw, so an unchanged object costs a `304` and nothing else.

Any S3-compatible store works: S3, R2, MinIO, SeaweedFS, RustFS, Garage.

- `pathStyle: true` for Garage and MinIO. AWS S3 and Cloudflare R2 use
  virtual-host style, so set `pathStyle: false` for them.
- `region` defaults to `auto`, which suits R2 and most self-hosted gateways.
  AWS needs the bucket's real region.

Scope the credential to **read** on that one object. See [security](/kallisto/explanation/security/)
for why write access to the bucket is the threat the design is built around.

## From local disk

```yaml
spec:
  source:
    type: disk
    path: /etc/kallisto/secrets.kal
```

For a file baked into an image, mounted from a Kubernetes `Secret`, or written
by another process on the machine. Polling and checks are identical. It needs
no bucket and no credentials, which also makes it the easiest source for local
development.

A disk source re-reads the file on every poll. The content version in the
header decides whether anything changes.

## Refresh interval

```yaml
spec:
  refresh:
    intervalSeconds: 30
```

30 seconds by default. The refresh loop runs on its own unpinned thread, so a
slow bucket call never occupies a core that is answering reads. Override it
with `--refresh-interval-seconds` or `KALLISTO_REFRESH_INTERVAL_SECONDS`.

## Fallback copy

```yaml
spec:
  cacheDir: /var/lib/kallisto
```

Kallisto keeps an encrypted copy of the last good file at
`<cacheDir>/secrets.kal`. It is never written in the clear. It does two jobs:

- **Cold start while the bucket is down.** A restarted resolver serves from the
  copy until the bucket answers again.
- **Anti-rollback across restarts.** On startup the resolver loads the copy
  *before* it first asks the bucket, so the version it already trusted becomes
  the floor. A restart is then no window for an older file.

Without `cacheDir` there is no copy, and a machine that restarts while the
source is unreachable stays sealed. `kallisto-ctl validate` warns about this.

If the directory is not writable, the server logs it on stderr and carries on
without a copy. In containers, make sure the volume is owned by uid 65532. See
[run as a sidecar](/kallisto/how-to/run-as-a-sidecar/).

## What the resolver refuses

- A file that fails authentication: wrong key, or altered bytes.
- A file whose content version is older than the one being served.
- A file that carries tokens but no `token_key`.

Each refusal leaves the current table in place, logs to stderr, and increments
`kallisto_refresh_failures_total`. Until a first file loads, every read answers
503, the Vault sealed state.

## Check the configuration

```bash
kallisto-ctl validate --config kallisto.yaml
```

```
kallisto.yaml: valid
  listen    127.0.0.1:8200
  workers   2
  mount     secret
  source    Bucket { endpoint: "https://s3.example.com", ... }
  refresh   every 30s
  cache     /var/lib/kallisto/secrets.kal
  limits    20000/s per worker, burst 20000
  log       on, queue 8192
```

It resolves the file alone, ignoring your shell's environment and flags, and
prints what the server would do. An unknown or misspelled field is an error.
Run it in CI before deploying a configuration change.

Every field is listed in the [configuration
reference](/kallisto/references/configuration/).
