# Naughtian Kallisto — a local, read-only secrets resolver that speaks Vault

<p align="center">
  <img src="https://img.shields.io/badge/Rust-2024-blue.svg?style=for-the-badge&logo=rust" alt="Rust 2024 edition">
  <img src="https://img.shields.io/badge/License-AGPLv3-red.svg?style=for-the-badge" alt="License">
</p>

Kallisto runs beside your application, on localhost, and answers Vault KV-v2 reads out of **one
encrypted file on an S3-compatible bucket**. It cannot write. There is no cluster, no database, no
replication and no admin API — those were all in earlier versions, and ADR-0015 deleted them.

Adopting it is one line:

```diff
- VAULT_ADDR=https://vault.internal:8200
+ VAULT_ADDR=http://127.0.0.1:8200
```

Removing it is the same line. That symmetry is the design constraint: if your application uses an
unmodified Vault SDK, it should not be able to tell.

## What it is for

One machine's applications need a handful of secrets and re-read them constantly. Pointing every one
of them at a central Vault means a network round trip on the request path and an outage whenever
that Vault is unreachable. Kallisto holds the current value locally, refreshes it on a timer, and
keeps serving from an encrypted on-disk copy when the bucket is down.

**It is not a Vault replacement.** It has no auth methods, no dynamic secrets, no leases, no PKI, no
transit engine. It reads one file. If you need any of that, run OpenBao — and put Kallisto in front
of it if you like.

## How it works

```
  operator                        bucket                      each machine
  ────────                        ──────                      ────────────
  kallisto-ctl seal   ──────►   secrets.kal   ──────►   kallisto-server :8200
    (AES-256-GCM,               (encrypted,             (polls, authenticates,
     version N)                  versioned)              refuses rollbacks,
                                                         serves KV-v2 reads)
                                                              │
                                                     app ─────┘  VAULT_ADDR=127.0.0.1
```

The file carries the secrets, the policies, and a table of keyed token hashes. **The policy table is
encrypted with everything else**, deliberately: permission to write to the bucket and possession of
the key are two different things, and a policy table in the clear would let whoever holds the first
grant themselves the second.

## Compatibility

| Vault call | Kallisto |
| --- | --- |
| `GET /v1/secret/data/<path>` | ✅ `metadata.version` is the file's content version |
| `GET /v1/secret/data/<path>?version=N` | ✅ if `N` is the current version, otherwise 404 |
| `GET`/`LIST` `/v1/secret/metadata/<path>` | ✅ both spellings; one entry in `versions` |
| `GET /v1/sys/health`, `/v1/sys/seal-status` | ✅ real state, plus `kallisto_file_version` and friends |
| `GET /v1/sys/mounts`, `/v1/auth/token/{lookup-self,renew-self}` | ✅ |
| `GET /v1/sys/metrics` | ✅ Prometheus text |
| `PUT`/`POST`/`PATCH`/`DELETE` on `data` | ❌ **403 `permission denied`** |
| `delete` / `undelete` / `destroy` / `subkeys` | ❌ **403 `permission denied`** |

The 403s are the product, not a gap. A resolver that cannot write is a resolver whose stolen
credentials are worth nothing.

**Version history is not kept.** The file holds current values only; `?version=N` answers for the
current version and 404s for anything else. Older values live in your git history and your bucket's
own versioning. The one counter Kallisto does keep is the file's content version, and it refuses any
file older than the one it already holds.

## What it does not protect against

Stated plainly, because a limit you have hidden is a trap:

- **Root on the machine, or anyone who can read the process's memory.** The in-RAM barrier keeps
  secrets encrypted between requests and zeroizes the decryption buffer after each response, which
  makes a core dump or a swap file far less rewarding. It does not stop a live debugger. Nothing at
  this layer can.
- **A response body in flight.** While a secret is being written into an HTTP response it is
  cleartext in this process. That is what serving a secret *is*.
- **Anyone holding the seal key.** They can read the file. The key belongs in your orchestrator's
  secret store, and never on a command line — arguments are world-readable through `ps`, which is
  why `--seal-key` is refused by name.
- **The access log is not an audit log.** It drops lines when its queue fills, so the machine keeps
  serving. It cannot tell you with certainty who read what. `kallisto_access_log_dropped_total`
  says when that happened; alert on it.

## Operational rules

1. **Never expose port 8200 beyond localhost.** There is no network authentication story here. The
   server refuses to start on a non-loopback address unless you pass `--i-accept-the-risk`.
2. **Applications must read secrets at runtime.** Do not bake them into a build artefact or a
   framework cache — Laravel's `config:cache` writes plaintext secrets into `bootstrap/cache`, and
   the whole point of this is lost the moment that happens.

## Getting started

```bash
make build

# A key, and a file to serve.
export KALLISTO_SEAL_KEY=$(cargo run -q -p kallisto-ctl -- gen-key)
cat > plain.json <<'JSON'
{"version": 1, "secrets": {"app/db": {"username": "admin", "password": "s3cr3t"}},
 "policies": {}, "tokens": {}}
JSON
cargo run -q -p kallisto-ctl -- seal --in plain.json --out secrets.kal

# Serve it.
cargo run -q -p kallisto-server -- --config kallisto.example.yaml
```

```bash
VAULT_ADDR=http://127.0.0.1:8200 vault kv get secret/app/db      # works
VAULT_ADDR=http://127.0.0.1:8200 vault kv put secret/app/db x=y  # 403, by design
curl -s localhost:8200/v1/sys/health | jq .kallisto_file_version
```

`kallisto.example.yaml` is committed and documents every setting. It contains no secret and has
nowhere to put one: keys come from the environment, and an invented field fails to parse rather than
being ignored.

## The offline tool

Everything that writes a sealed file happens in `kallisto-ctl`, never over the network.

| Command | |
| --- | --- |
| `seal --in plain.json --out secrets.kal [--version N]` | Encrypt. Refuses to write a version the resolver would reject as a rollback. |
| `verify --in secrets.kal` | Check the tag; report counts. Never prints a secret or a path. |
| `bump-version --in secrets.kal [--to N]` | Raise the content version in place. |
| `mint-token [--in secrets.kal] --policy <name>` | Generate a token and the table row to paste. Shown once. |
| `gen-key` | 32 bytes from the system RNG. |
| `validate --config kallisto.yaml` | Parse and resolve; print what the server would do. |
| `open --in secrets.kal --yes-print-secrets-to-stdout` | Exactly as dangerous as it sounds. |

## Status

A prototype under active rework. Not production-ready, and version 1.x makes no stability promise —
see ADR-0015 for what changed and why. Do not run this where it matters yet.

`Naughtian Kallisto` is AGPLv3. A commercial licence can be discussed.

## Building and testing

```bash
make dev       # fmt + clippy + cargo-deny + tests, in CI's order
make verify    # ADR-0013's blocking verification suite
make duck      # three real Vault SDKs against the real server
make bench-duck
```

Building needs `cmake` and `clang`, for `aws-lc-rs` — the only native dependency left.

## Documentation

The `docs/` directory is a Hugo (Hextra) site. The decisions that shaped this are in
`docs/content/docs/references/ADRs/`; ADR-0015 and ADR-0016 are the ones that produced the current
design, and `docs/content/docs/references/verification-status.md` records what is actually proven
versus merely believed.
