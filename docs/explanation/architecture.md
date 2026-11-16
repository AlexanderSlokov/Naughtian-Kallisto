---
title: Architecture
description: How one kallisto-server process refreshes, holds and serves a sealed file, and why 2.0.0 took this shape.
sidebar:
  order: 1
---

Kallisto 2.0.0 is one process with one port. A refresh loop pulls a sealed file
from a source, a snapshot holds what it decrypted, and a set of pinned workers
answer Vault KV-v2 reads from that snapshot on loopback.

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
│   │ limiter │    │ limiter │      │ limiter │  current_thread         │
│   │ log     │    │ log     │      │ log     │  runtime each;          │
│   │ counters│    │ counters│      │ counters│  nothing shared         │
│   └────┬────┘    └────┬────┘      └────┬────┘                         │
│        └──────────────┴────────────────┘                              │
│                       │ SO_REUSEPORT                                  │
└───────────────────────┼───────────────────────────────────────────────┘
                        ▼
              port 8200, loopback only
```

## The refresh loop

The refresh loop runs on its own thread and runtime, deliberately left
unpinned. A bucket request can take up to its 15-second timeout, and that wait
must never occupy a core that is answering reads. It polls every 30 seconds by
default, and each poll follows four steps:

1. Ask the source whether the sealed file changed. A bucket poll sends the last
   ETag, so an unchanged object costs a `304`.
2. If it changed, authenticate it and check that its content version is not
   older than the one being served.
3. If it is valid, swap the whole table in. If not, keep the old table, log the
   refusal and count it.
4. The workers answer from whatever table is in place.

On startup the loop loads the encrypted fallback copy on local disk *before* it
first asks the source. The version it finds becomes the floor for the rollback
check, so a restart gives an attacker no opening, and a machine can boot while
the bucket is down.

The source is a real port with two adapters, bucket and disk. It stays a port
because a third source is plausible, and at two calls a minute the dynamic
dispatch costs nothing.

## The snapshot

The decrypted file becomes a `Snapshot`, stored in an `ArcSwapOption` and
replaced whole. A request never sees a half-updated table, and a bad file
leaves the previous one exactly where it was. `None` is Vault's sealed state:
every read answers 503 until the first file loads.

The snapshot is a plain `HashMap`, sized for a few dozen secrets read almost
exclusively. The project's own measurements put the time in the HTTP layer, so
a cleverer structure would buy nothing. Paths are also kept sorted, so a LIST
finds its prefix by binary search.

Inside the snapshot, each secret is sealed individually under a key that exists
only in this process and only for this snapshot. A request opens exactly the
one secret it asked for, into a buffer owned by its worker thread, builds the
response inside that window, and the buffer is wiped. A new file brings a new
key, and the old key is zeroed when the old snapshot drops. See
[security](/kallisto/explanation/security/) for what this is worth.

## The workers

Each worker is a single-threaded Tokio runtime on its own thread, pinned to a
core with `core_affinity`. All workers share port 8200 through `SO_REUSEPORT`,
and the kernel balances connections between them. There is no work-stealing.

The router is built once per worker, on that worker's thread. The rate limiter,
the access-log producer and the metrics counters therefore each belong to one
core, and the read path never touches a cache line another core writes. The
rate limit is per worker for this reason, and the configuration field is named
`requestsPerSecondPerWorker` to say so.

A handler loads the current snapshot and reads a `HashMap`. There is no engine
trait, no registry and no virtual call on the read path. An earlier abstraction
here charged a dynamic dispatch per request for a substitutability nobody
needed, so 2.0.0 removed it.

## Why the read path is treated as hot

ADR-0015 first assumed one thread was enough, since most applications read
their secrets once at startup. ADR-0016 reversed that for one real pattern:
fetching a secret right before each use and discarding it right after. That
pattern keeps secrets out of application memory for as long as possible, and it
turns every use of a secret into a request. For those applications the read
path is the hot path, so it is built like one.

Applications that read once at startup lose nothing: set `workers: 1`.

## Authorization

A token table travels inside the sealed file, as keyed hashes mapped to policy
names. Lookup is constant-time and visits every entry. Policies use Vault's path
syntax. A file with no token table permits every read, which suits one app with
its own file. Any file with a table is default deny.

## Telemetry

One access-log line and one counter increment per request, emitted from a
single middleware layer so that no route can bypass it. Lines travel over a
lock-free queue to a writer thread, and when the queue is full they are dropped
and counted. stdout carries the access log only. Operational messages go to
stderr.

## What the process does not have

One plane and one port. There is no admin API, no port 8202, no cluster, no
gossip, no storage engine and no write path. The workspace is Rust only.

## How it got here

2.0.0 is the third architecture:

1. **A C++ core behind an FFI bridge.**
2. **A pure-Rust secrets server**, with a sharded cuckoo table, a B-tree index,
   RocksDB storage, a KV-v2 write path, an admin port and a planned gossip
   controlplane.
3. **The resolver.** ADR-0015 changed the question from "how do we build a fast
   secrets server" to "what does one machine's application actually need". The
   answer was much smaller, and about nine thousand lines were removed.

Each deletion also retired a class of risk. With no storage engine there is
no durability bug to have, and with no write endpoint there is no endpoint
that can be tricked into writing. The Kallisto repository keeps the full account under
`docs/explanation/how-to-create-naughtian-kallisto/`.

## The one-line contract

```diff
- VAULT_ADDR=https://vault.internal:8200
+ VAULT_ADDR=http://127.0.0.1:8200
```

Removing it is the same line. A component that is painful to remove is one
nobody dares to experiment with, and it becomes permanent by accident. The same
principle runs through the ecosystem: remove [Helvilette](/helvilette/) and you
still have working Ansible playbooks, ignore a [Kuberina](/kuberina/) blueprint
and your cluster schedules normally.
