# Configuring the source

Kallisto reads one sealed file. `spec.source` says where it comes from. There are two kinds, and a
third is plausible — which is why this is a real port in the code (`SecretSource`) rather than an
`if`. It runs twice a minute, so the dynamic dispatch costs nothing.

This is not storage configuration. Kallisto stores nothing; it polls a file somebody else wrote.

## A bucket

```yaml
spec:
  source:
    type: bucket
    endpoint: https://s3.example.com
    bucket: kallisto
    objectKey: prod/payment.kal
    region: auto
    pathStyle: true
```

Any S3-compatible bucket: S3, R2, MinIO, SeaweedFS, RustFS, Garage. Requests are SigV4-signed, with
a 15-second timeout.

`pathStyle: true` for Garage and MinIO; R2 and AWS take virtual-host style. `region` defaults to
`auto`, which is what R2 and most self-hosted gateways want; AWS needs the real region.

Credentials come from the environment and nowhere else:

```
KALLISTO_S3_ACCESS_KEY_ID
KALLISTO_S3_SECRET_ACCESS_KEY
```

They cannot be written in this file. There is no field for them, and a key field here fails to
parse rather than being silently ignored.

Give the credential read-only access to the one object. Write access to the bucket is the attacker
position Kallisto's anti-rollback check exists to address — see below.

## A local file

```yaml
spec:
  source:
    type: disk
    path: /etc/kallisto/secrets.kal
```

For a file baked into the image, mounted as a Kubernetes `Secret`, or written by something else on
the machine. Same polling and same checks; the only difference is where the bytes come from.

Useful for local development, since it needs no bucket and no credentials.

## The seal key

```
KALLISTO_SEAL_KEY
```

32 bytes, hex-encoded. Required, from the environment only. Never a flag — command-line arguments
are readable by every process on the host through `ps` — and never in the configuration file.
Passing `--seal-key` exists solely to reject it with that explanation.

```bash
export KALLISTO_SEAL_KEY=$(cargo run -q -p kallisto-ctl -- gen-key)
```

## Refresh and the fallback copy

```yaml
spec:
  refresh:
    intervalSeconds: 30
  cacheDir: /var/lib/kallisto
```

Every 30 seconds by default, on a thread of its own that is deliberately not pinned: a
fifteen-second bucket timeout must never occupy a core that is answering reads.

`cacheDir` holds an encrypted copy of the last good file, written as `secrets.kal`. It is what lets
an application start when the bucket is down, and it is never written in the clear. On startup the
resolver warms from it *before* the first bucket poll, so the version it finds becomes the floor
for the anti-rollback check — without that, a restart is the moment a rollback would succeed.

If the copy cannot be written, the server says so on the error log and carries on; the consequence
is that the machine starts sealed if the source is unreachable.

## What the resolver refuses

A file that fails authentication, and a file whose content version is older than the one already
being served. Both leave the previous snapshot exactly where it was and increment
`kallisto_refresh_failures_total`.

The anti-rollback check has to be in-process because the attacker in the threat model is whoever
can write to the bucket: bucket versioning is under their control, so only a check on this side
helps.

Until the first file loads, every read answers 503 — Vault's sealed state, not an empty 200.

## Checking a configuration without starting the server

```bash
kallisto-ctl validate --config kallisto.yaml
```

Parses and resolves it, and prints what the server would actually do. A misspelled field is an
error rather than a silent default.

Precedence, for every setting: command line, then environment, then this file, then the default.
