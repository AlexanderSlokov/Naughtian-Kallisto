# Kallisto demo

A local environment that runs the real deployment shape: an S3 bucket (Garage), a sealed file
uploaded into it, and Kallisto polling that bucket and serving Vault KV-v2 reads.

Nothing is compiled — every image is pulled, so no Rust toolchain is needed. Every image is pinned
to a version. The bucket used to be MinIO at `latest`, and the demo broke when MinIO stopped
publishing images (issue #61).

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

Garage has no web console. To see `demo.kal`, the AES-256-GCM file Kallisto pulled, list the
bucket with the rclone that is in the init image:

```bash
docker compose -f docker-compose.demo.yml --env-file .env.demo \
  run --rm --entrypoint rclone kallisto-init lsl demo:kallisto-demo
```

Replace `lsl demo:kallisto-demo` with `cat demo:kallisto-demo/demo.kal | head -c 64 | xxd` to look
inside: it starts with `KALLISTO` and the rest is ciphertext. The plaintext it was sealed from is
`plain.demo.json`.

## What's running

```
Garage (:3900 S3, loopback only)
  → kallisto-init [one-shot]
      gives Garage a layout, a key and the bucket,
      seals plain.demo.json → demo.kal, uploads
  → kallisto (:8200)
      polls bucket every 5s, serves KV-v2 reads
```

`"sealed": false` in the health response means it loaded the file. `kallisto_file_version` is the
content version inside the sealed file, and `"kallisto_authorization": "none"` means this file
carries no token table, so every read is permitted — the single-app sidecar shape.

`docker compose ps` shows `kallisto` as `healthy` once it has loaded the file. The check is
`kallisto-ctl health --ready`, run inside the container: the image is distroless, and that binary is
the only thing in it that can ask.

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
   `http://garage:3900` instead of `127.0.0.1:3900`. Garage must then bind `[::]:3900` and
   `[::]:3901` in `garage.toml`, as `k8s/bucket.yaml` does.
2. Kallisto must then bind `0.0.0.0` *inside its container*, which requires
   `--i-accept-the-risk`. Publish it as `127.0.0.1:8200:8200` so the host-side exposure is still
   loopback only.

That second step is a real trade and worth understanding before copying it: the flag exists
because binding a routable address turns a sidecar into an unauthenticated secrets endpoint. In
this shape the container's network namespace is the boundary instead of the loopback interface.

## Credentials

`.env.demo` holds deliberately fake placeholders — the seal key is thirty-two zero bytes. Nothing
in it is a credential, which is why no scanner exclusion is needed for it.

The bucket key is zeros as well, but not free-form: Garage requires a key id of `GK` followed by 24
hex characters and a secret of 64 hex characters, and refuses anything else.

Generate a real key with:

```bash
cargo run -p kallisto-ctl -- gen-key
```

## Tear down

```bash
docker compose -f docker-compose.demo.yml --env-file .env.demo down -v
```

`-v` removes Garage's data and Kallisto's encrypted fallback copy. Without it, the next `up`
starts from the file already in the bucket.

Use `-v` in particular after changing `KALLISTO_SEAL_KEY`. The fallback copy is sealed under the
old key, and on the next start the resolver refuses it — correctly — with
`refused the file offered by cache: sealed file failed authentication`. It then loads from the
bucket and serves normally, so the line is alarming but harmless.

## Where to read more

- [What Kallisto is](../docs/index.mdx)
- [Architecture](../docs/explanation/architecture.md)
- [Configure the source](../docs/how-to/configure-the-source.md)
- [Security and the threat model](../docs/explanation/security.md)
