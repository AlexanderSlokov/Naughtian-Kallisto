---
title: Run the demo
description: Start MinIO, seal a file into it, and watch Kallisto serve and refresh Vault KV-v2 reads.
sidebar:
  order: 1
---

By the end of this tutorial you will have the real deployment shape running on
your machine: an S3 bucket (MinIO), a sealed secrets file uploaded into it, and
Kallisto polling that bucket and answering Vault KV-v2 reads on
`127.0.0.1:8200`. You will read a secret, watch a write get refused, and push a
new version without restarting anything.

:::caution[Demo credentials only]
The demo's seal key is thirty-two zero bytes and the MinIO credentials are
placeholders. They are committed on purpose. Never reuse them.
:::

## Before you start

You need:

- **Linux.** Every demo service uses `network_mode: host`. Kallisto only binds
  loopback, and a container's own loopback is unreachable from the host, so the
  demo shares the host's network instead. Docker Desktop on macOS and Windows
  does not provide host networking the same way.
- **Docker** with the Compose v2 plugin.
- **Git**, to fetch the demo files.
- `curl`, and `python3` to pretty-print JSON.

No Rust toolchain is needed. The images are pulled from GHCR.

## Step 1: Get the demo files

```bash
git clone https://github.com/AlexanderSlokov/Naughtian-Kallisto.git
cd Naughtian-Kallisto/demo
```

The directory holds a Compose file, a Kallisto configuration, a plaintext
secrets file (`plain.demo.json`) and a one-shot init script.

## Step 2: Start it

```bash
docker compose -f docker-compose.demo.yml --env-file .env.demo up
```

Three things happen in order:

1. **MinIO** starts on `127.0.0.1:9000`, with its console on `:9001`.
2. **kallisto-init** runs once. It creates the `kallisto-demo` bucket, seals
   `plain.demo.json` into `demo.kal` with `kallisto-ctl`, and uploads it.
3. **kallisto** starts, polls the bucket every 5 seconds, and serves on
   `127.0.0.1:8200`.

Give it about 20 seconds.

## Step 3: Check that it is serving

In a second terminal:

```bash
curl -s http://127.0.0.1:8200/v1/sys/health | python3 -m json.tool
```

Look for three fields:

- `"sealed": false` means a file has loaded. Before the first file loads,
  Kallisto reports itself sealed and every read answers 503.
- `"kallisto_file_version": 1` is the content version inside the sealed file.
- `"kallisto_authorization": "none"` means the file carries no token table, so
  every read is permitted. That is the single-app sidecar shape, and the demo
  uses it on purpose.

## Step 4: Read and list

```bash
# Read one secret
curl -s http://127.0.0.1:8200/v1/secret/data/app/database | python3 -m json.tool

# List everything under app/
curl -s 'http://127.0.0.1:8200/v1/secret/metadata/app/?list=true' | python3 -m json.tool
```

The listing needs `?list=true`, the way Vault spells a LIST over GET.
`curl -X LIST` works too. A plain GET on `metadata/app/` asks for that path's
metadata and answers 404.

If you have the `vault` CLI installed, it works unchanged:

```bash
VAULT_ADDR=http://127.0.0.1:8200 vault kv get secret/app/database
```

## Step 5: Try to write

```bash
curl -s -o /dev/null -w '%{http_code}\n' -X PUT \
  http://127.0.0.1:8200/v1/secret/data/app/database
```

It prints `403`. Kallisto has no write path. A resolver that cannot write is a
resolver whose stolen credentials are worth nothing, so the door exists and is
shut.

## Step 6: Push a new version

Open `plain.demo.json`. Change a password, and **raise `"version"` from `1` to
`2`**. Then re-run the init container to re-seal and re-upload:

```bash
docker compose -f docker-compose.demo.yml --env-file .env.demo run --rm kallisto-init
```

Within 5 seconds the health check shows `"kallisto_file_version": 2` and the
read in step 4 returns the new value. Nothing restarted.

Raising the version is required. Content versions are monotonic and never
reused, so a file offering a version Kallisto already holds is treated as the
same file and never opened. Edit a value without bumping the version and you
will see no change, which is the correct behaviour. A file with a *lower*
version is refused as a rollback.

## Step 7: Look inside the bucket

Open the MinIO console at `http://localhost:9001` and log in with
`demo-access-key` / `demo-secret-key`. The `kallisto-demo` bucket holds
`demo.kal`. Download it and you get AES-256-GCM ciphertext. The plaintext it
came from is the `plain.demo.json` beside you.

## Tear down

```bash
docker compose -f docker-compose.demo.yml --env-file .env.demo down -v
```

`-v` removes the MinIO data and Kallisto's encrypted fallback copy. Use it in
particular after changing `KALLISTO_SEAL_KEY`: the old fallback copy was sealed
under the old key, and on the next start Kallisto refuses it with
`sealed file failed authentication`, then loads from the bucket. The message is
alarming and harmless.

## What you have seen

- An operator seals a file offline and puts it in a bucket.
- Each machine's resolver polls, authenticates and serves it on loopback.
- Writes are refused, and new content arrives by re-sealing with a higher
  version.

## Where to go next

- [Seal and publish a secrets file](/kallisto/how-to/seal-and-publish-a-file/)
  with a real key.
- [Run Kallisto as a sidecar](/kallisto/how-to/run-as-a-sidecar/) next to your
  own application.
- [Architecture](/kallisto/explanation/architecture/) for what happens inside
  the process.
