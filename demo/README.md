# Kallisto demo

A local environment that runs the real deployment shape: an S3 bucket (MinIO), a sealed file
uploaded into it, and Kallisto polling that bucket and serving Vault KV-v2 reads.

Nothing is built from source — both images are pulled, so no Rust toolchain is needed.

## Before you start

**This demo needs Linux.** Every service uses `network_mode: host`, because Kallisto refuses to
bind anything but loopback and a container's own loopback is not reachable from the host. Docker
Desktop on macOS and Windows does not provide host networking the same way, so the demo is not
expected to work there as written. See [Running without host networking](#running-without-host-networking).

## Run it

```bash
docker compose -f docker-compose.demo.yml --env-file .env.demo up
```

Wait ~20s for init to finish, then:

```bash
# Is Kallisto serving?
curl -s http://127.0.0.1:8200/v1/sys/health | python3 -m json.tool

# Read a secret
curl -s http://127.0.0.1:8200/v1/secret/data/app/database | python3 -m json.tool

# List secrets under app/ — note ?list=true, the way Vault spells a LIST over GET
curl -s 'http://127.0.0.1:8200/v1/secret/metadata/app/?list=true' | python3 -m json.tool

# Writing is refused — that is the product, not a missing feature
curl -s -o /dev/null -w '%{http_code}\n' -X PUT http://127.0.0.1:8200/v1/secret/data/app/database
```

The last one prints `403`.

A plain `GET` on `metadata/app/` without `?list=true` answers 404, not a listing — Vault
distinguishes reading a path's metadata from listing under it, and `curl -X LIST` works too.

MinIO UI at http://localhost:9001 (demo-access-key / demo-secret-key). Open the `kallisto-demo`
bucket to see `demo.kal` — the AES-256-GCM file Kallisto pulled. Downloading it gets you
ciphertext; the plaintext it was sealed from is `plain.demo.json`.

## What's running

```
MinIO (:9000 S3, :9001 UI)
  → kallisto-init [one-shot]
      creates bucket, seals plain.demo.json → demo.kal, uploads
  → kallisto (:8200)
      polls bucket every 5s, serves KV-v2 reads
```

`"sealed": false` in the health response means it loaded the file. `kallisto_file_version` is the
content version inside the sealed file, and `"kallisto_authorization": "none"` means this file
carries no token table, so every read is permitted — the single-app sidecar shape.

## See a refresh happen

The demo polls every 5 seconds instead of the usual 30, so a change is visible while you watch.

Edit `plain.demo.json` — change a password, and **raise `"version"` to `2`** — then re-run the
init container to re-seal and re-upload:

```bash
docker compose -f docker-compose.demo.yml --env-file .env.demo run --rm kallisto-init
```

Within 5 seconds `kallisto_file_version` moves to 2 and the new value is served. Nothing restarts.

Raising the version is not a formality. Content versions are monotonic and never reused, so a file
offering a version Kallisto already holds is treated as the same file and **is not even opened** —
which is also what stops a same-numbered forgery from displacing the table already being served.
Edit a value without bumping the version and you will watch nothing happen, correctly.

Going the other way is refused outright: seal a *lower* version and the resolver rejects it as a
rollback and keeps what it has. `kallisto-ctl seal` will not even write one without `--force`,
which is why the init script passes that flag — it re-seals version 1 over itself every time the
container is restarted.

## Running without host networking

If you are on Docker Desktop, or you cannot use `network_mode: host`, the demo needs two changes
and they are not made for you here because they have not been tested on those platforms:

1. Put the services on a bridge network and point `source.endpoint` in `kallisto.demo.yaml` at
   `http://minio:9000` instead of `127.0.0.1:9000`.
2. Kallisto must then bind `0.0.0.0` *inside its container*, which requires
   `--i-accept-the-risk`. Publish it as `127.0.0.1:8200:8200` so the host-side exposure is still
   loopback only.

That second step is a real trade and worth understanding before copying it: the flag exists
because binding a routable address turns a sidecar into an unauthenticated secrets endpoint. In
this shape the container's network namespace is the boundary instead of the loopback interface.

## Credentials

`.env.demo` holds deliberately fake placeholders — the seal key is thirty-two zero bytes. Nothing
in it is a credential, which is why no scanner exclusion is needed for it.

Generate a real key with:

```bash
cargo run -p kallisto-ctl -- gen-key
```

## Tear down

```bash
docker compose -f docker-compose.demo.yml --env-file .env.demo down -v
```

`-v` removes the MinIO data and Kallisto's encrypted fallback copy. Without it, the next `up`
starts from the file already in the bucket.

Use `-v` in particular after changing `KALLISTO_SEAL_KEY`. The fallback copy is sealed under the
old key, and on the next start the resolver refuses it — correctly — with
`refused the file offered by cache: sealed file failed authentication`. It then loads from the
bucket and serves normally, so the line is alarming but harmless.

## Where to read more

- [What Kallisto is](../docs/explanation/what-is-kallisto.md)
- [Architecture](../docs/explanation/architecture.md)
- [Configuring the source](../docs/operations/configuration/source.md)
- [Security and the threat model](../docs/explanation/security.md)
