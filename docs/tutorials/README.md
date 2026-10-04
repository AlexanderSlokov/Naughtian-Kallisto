# Tutorials

## Run Kallisto locally

[`demo/`](../../demo/) is a working local environment: MinIO standing in for the bucket, a sealed
file uploaded into it, and Kallisto polling that bucket and serving Vault KV-v2 reads. Both images
are pulled, so it needs no Rust toolchain — but it does need Linux, for the reason its README
explains.

```bash
cd demo
docker compose -f docker-compose.demo.yml --env-file .env.demo up
```

It is the fastest way to see the real deployment shape, including a live refresh and a write being
refused with a 403.

## Elsewhere

The hosted quickstart is at
[docs.naughtian.org/kallisto/tutorials/quickstart](https://docs.naughtian.org/kallisto/tutorials/quickstart/),
which is the front door for people running Kallisto rather than changing it.

For what Kallisto is before you run anything, start at
[what-is-kallisto.md](../explanation/what-is-kallisto.md).
