# Architecture Overview

Kallisto is a local, read-only secrets resolver that provides Vault KV-v2 over one encrypted file on
an S3-compatible bucket. It is important to note that Kallisto is not a secrets server: 
it stores nothing, replicates nothing, and answers 403 to every route that writes.

ADR-0015 redefined the problem from "a high-performance secrets server" to "a resolver for one
machine's apps", and ADR-0016 amended two of its details after the read path turned out to be hot.
The shape below is the result. For why the two previous architectures were abandoned, see
[Architecture in Depth](./how-to-create-naughtian-kallisto/architecture-in-deep.md).

## The shape

```
                      S3-compatible bucket (or local disk)
                                    │  secrets.kal, AES-256-GCM
                                    ▼
┌───────────────────────────────────────────────────────────────────────┐
│                    kallisto-server (one process)                      │
│                                                                       │
│   refresh thread (own current_thread runtime, NOT pinned)             │
│   ┌────────────────────────────────────────────────────────────┐      │
│   │ SecretSource  ─►  authenticate  ─►  anti-rollback  ─► build│      │
│   │ (bucket │ disk)   AES-256-GCM       refuse older file      │      │
│   └──────────────────────────────┬─────────────────────────────┘      │
│                                  │ store whole snapshot               │
│                                  ▼                                    │
│                  ┌──────────────────────────────┐                     │
│                  │  SnapshotSlot                │                     │
│                  │  ArcSwapOption<Snapshot>     │                     │
│                  │  None = sealed = 503         │                     │
│                  │  HashMap<path, Sealed>       │                     │
│                  │  + per-snapshot barrier key  │                     │
│                  └──────────────┬───────────────┘                     │
│                        load (no lock, no dyn)                         │
│        ┌──────────────┬─────────┴──────┐                              │
│        ▼              ▼                ▼                              │
│   ┌─────────┐    ┌─────────┐      ┌─────────┐                         │
│   │Worker 0 │    │Worker 1 │      │Worker N │  pinned, one            │
│   │ axum    │    │ axum    │      │ axum    │  current_thread         │
│   │ limiter │    │ limiter │      │ limiter │  runtime each;          │
│   │ log prod│    │ log prod│      │ log prod│  nothing shared         │
│   │ counters│    │ counters│      │ counters│  on the read path       │
│   └────┬────┘    └────┬────┘      └────┬────┘                         │
│        └──────────────┴────────────────┘                              │
│                       │ SO_REUSEPORT — kernel balances connections    │
└───────────────────────┼───────────────────────────────────────────────┘
                        ▼
              port 8200, loopback only
```

There is one plane and one port. Port 8202, the admin server, the FFI bridge to a C++ core and the
gossip cluster are gone — as are the storage engine, the RocksDB backend, the sharded cuckoo table
and the registry of pluggable engines. The workspace is Rust only.

## The data plane

Port 8200, bound to loopback. `config` refuses a non-loopback bind unless the operator
passes `--i-accept-the-risk` flag, because ADR-0015's first operating constraint is that this port is not
reachable from the network.

Each worker is one `current_thread` Tokio runtime on its own thread, pinned with `core_affinity`,
and several workers share the one port through `SO_REUSEPORT` (ADR-0016 QĐ-3). There is no
work-stealing. The router is built once per worker, on that worker's own thread, so the rate
limiter, the access-log producer and the metrics counters each belong to one core and the read path
never touches a shared cache line. Two workers unless configured otherwise.

The output port is welded shut (ADR-0016 QĐ-4). There is no `SecretEngine` trait, no
`EngineRegistry`, no `Arc<dyn>` on the read path: a handler loads the current snapshot and reads a
`HashMap`. The abstraction that used to sit here charged a virtual call per request for a
substitutability nobody asked for.

Every write route answers 403, and `subkeys`, `delete`, `undelete` and `destroy` are routed
explicitly so they answer 403 rather than 404 — a 404 would read as "not deployed yet", where 403
says the door exists and is shut.

Alongside the KV surface, `sys/health`, `sys/init`, `sys/metrics`, `sys/seal-status`, `sys/mounts`,
`sys/internal/ui/mounts/*` and `auth/token/{lookup-self,renew-self}` exist so an unmodified Vault
SDK can start up (ADR-0015 D7). None of them serve a secret, and `sys/health` reports the version
actually loaded rather than a string literal.

## The refresh loop

Its own thread and its own runtime, deliberately not pinned: a fifteen-second bucket timeout must
never occupy a core that is answering reads. It polls every thirty seconds by default.

`SecretSource` is a real port with two implementations — an S3 bucket (SigV4-signed) and a local
file. It was kept as a port because a third source is plausible and it runs twice a minute, so
dynamic dispatch costs nothing there.

Four steps, in order (ADR-0015 D10):

1. ask the source whether the sealed file changed;
2. if it did, open it, authenticate it, and check it is not older than what is already served;
3. valid file, swap the whole table in; invalid file, keep the old table and say so on the error log;
4. the HTTP layer answers out of whatever table is currently in place.

On startup the loop warms from the encrypted copy on local disk first, so the version it finds
becomes the floor for the anti-rollback check before the bucket is ever asked. Every poll is
reported: discarding the later ones is how a forged file in the bucket would become invisible after
the first thirty seconds. A rejection or an outage also increments
`kallisto_refresh_failures_total`, so "this machine has been serving a stale file for an hour" is
something a scrape notices.

## The snapshot

`ArcSwapOption<Snapshot>`, swapped in whole. A request never sees a half-updated table, and a bad
file leaves the previous one exactly where it was. `None` is Vault's sealed state and answers 503,
not an empty 200.

ADR-0015 D11 sized this deliberately: a few dozen secrets, read almost exclusively, so a plain
`HashMap` behind `arc-swap` is enough — the project's own benchmark put the time in the HTTP layer,
not the lookup. Paths are also held sorted so a LIST finds its prefix range by binary search instead
of scanning.

Secrets are sealed individually in RAM under a per-snapshot AES-256-GCM key that is never written
anywhere, and opened into a `thread_local` buffer for the length of one response (ADR-0015 D13). The
response body is built inside that callback, so the cleartext exists for exactly as long as it takes
to copy it into the body; the buffer is wiped when the closure returns. A new file means a new key,
and the old one is zeroed when the old snapshot drops.

## The file, and what makes it trustworthy

The sealed file is `KALLISTO` magic, a format version, the content version as a little-endian
`u64`, a 12-byte nonce, then AES-256-GCM ciphertext. The header is plaintext so the version can be
checked before anything is decrypted, and it is passed as additional authenticated data, so editing
any of it — the content version most of all — fails authentication.

Anti-rollback matters because the attacker in the threat model is whoever can write to the bucket.
Bucket versioning is therefore under their control, and only an in-process check helps: a file whose
version is older than the one held is refused.

The 32-byte seal key comes from the environment, never from the config file and never from a flag,
because command-line arguments are world-readable through `ps`. Everything that *writes* a sealed
file happens offline in `kallisto-ctl`, never over the network. Before any secret is read the process
drops core dumps and refuses `ptrace`.

## Authorization

A token table built from the file, looked up in constant time, with Vault-style path matching.

A file that carries no token key and no tokens is the sidecar deployment of ADR-0015 D8: one app,
its own file, and the bucket credential is the boundary. Every read is then permitted, and
`sys/health` reports that rather than leaving an operator to discover it. Anything else is default
deny, including a request that carried no token. A file carrying tokens but no key to check them
against is refused outright, rather than served with authorization quietly switched off.

Overload is answered the way Vault answers it — 429 with `Retry-After` — from a token bucket per
worker. The configured rate is therefore per worker, and the configuration field says so in its
name; a process-wide limiter would put a contended atomic in front of every read.

## Telemetry, which is not an audit log

One access-log line and one counter per request, emitted from a single middleware layer rather than
from each handler: an access log's whole value is that every request appears in it, and a layer
cannot be bypassed by adding a route. Lines go over a lock-free queue to a writer thread, and stdout
carries the access log and nothing else — operational messages go to stderr.

This is not an audit log, and a test enforces that nothing in the telemetry crate is named one. The
distinction is the design (ADR-0015 D15): an audit log records *before* it serves, so a full queue
means refusing to serve, whereas this records after the fact and drops when it falls behind. Calling
a drop-on-full log "audit" would let somebody believe they can answer "who read the Stripe key" when
they cannot.

Paths and tokens are written as keyed hashes, never as themselves, and no secret value is written at
all — the error log included. Identifiers are computed when the file loads rather than per request,
because an HMAC costs roughly what the whole barrier costs, and paying one per request would make
observing a read more expensive than serving it. The log key is derived from the file's own token
key so identifiers survive a restart; with no token table it falls back to a key that lives only as
long as the process, and the startup log says so.
