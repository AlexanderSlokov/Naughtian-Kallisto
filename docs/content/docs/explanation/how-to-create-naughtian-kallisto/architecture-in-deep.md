---
title: "Architecture in Depth"
weight: 30
---

This page explains **why the system has the shape it has**. It is the third architecture Kallisto
has had, and the previous two are worth a paragraph each, because the reasons they were abandoned
are the reasons this one is built the way it is.

- **A C++ core with a Rust FFI bridge.** `cxx::bridge`, Corrosion, BoringSSL, a `ValidEngine`
  C++20 concept. Two languages, two build systems, and a bridge to keep in step across both.
- **A pure-Rust secrets *server*.** Hexagonal with an `ISecretEngine` port, an `EngineRegistry`
  router, a `KallistoCore` facade, a sharded cuckoo cache, a thread-local B-tree path index, a
  RocksDB backend behind a lock-free write-behind queue, and a gossip control plane on port 8202.
  It worked. ADR-0015 deleted almost all of it.

ADR-0015 changed the question. Not "how do we build a fast secrets server" but "what does one
machine's applications actually need", and the answer was much smaller: **a local, read-only
resolver that speaks Vault KV-v2 over one encrypted file on an S3-compatible bucket.** Roughly nine
thousand lines came out in a single commit.

## The shape

```
  operator                     bucket                    each machine
  ────────                     ──────                    ────────────
  kallisto-ctl seal  ──────►  secrets.kal  ──────►  kallisto-server :8200
    AES-256-GCM,              encrypted,             polls, authenticates,
    version N                 versioned              refuses rollbacks,
                                                     serves KV-v2 reads
                                                          │
                                                 app ─────┘  VAULT_ADDR=127.0.0.1
```

Four modules and nothing else: `config`, `resolver`, `server`, `event`.

## Asymmetric hexagonal

ADR-0015 D10 wanted the hexagon gone entirely. ADR-0016 QĐ-4 amended that into two different
answers for the two sides, because the two sides are not alike.

**The output port is welded shut.** There is no `SecretEngine` trait, no registry, no `Arc<dyn>` on
the read path. A handler loads the current snapshot and reads a `HashMap`. Every layer of
abstraction here was an indirect call paid for on every request, bought against a substitutability
nobody had asked for.

**The source port is a real port.** `SecretSource` has two implementations — an S3 bucket and a
local file — because a third source is genuinely plausible, and it runs twice a minute, so dynamic
dispatch costs nothing measurable.

If welding the output side ever makes a change hard, the seam is known: it is the boundary between
`vault_api.rs` and `Snapshot`.

## Thread-per-core, kept on purpose

ADR-0015 D9.1 said to drop CPU pinning and run one thread, on the reasoning that one application
needs very little. ADR-0016 QĐ-3 reversed it, because that premise is false for a real class of
application: some fetch a secret immediately before each use and discard it, caching nothing. For
those, the read path *is* the hot path.

So the Envoy-shaped layout stays: one `current_thread` Tokio runtime per worker, pinned with
`core_affinity`, several workers on one port through `SO_REUSEPORT`, the kernel distributing
connections. No work-stealing, no cross-core cache traffic.

The consequence that shapes everything downstream: **anything a worker touches per request belongs
to that worker alone.** The rate limiter, the decryption scratch buffer, the access-log producer and
the metrics counters are all per worker, so the read path never contends a shared cache line. The
configured rate limit is therefore *per worker*, and the field name says so.

The refresh loop gets its own thread and runtime and is deliberately **not** pinned: a slow bucket
call must never occupy a core that is answering reads.

## The resolver

`Snapshot` lives in an `ArcSwapOption`. `None` *is* Vault's sealed state and answers `503`. A file
that fails to parse, fails its tag, or is older than the one held leaves the previous snapshot
exactly where it was — the machine keeps serving (D14).

**Anti-rollback is the interesting part.** The file's content version sits in a plaintext header
that is fed to the AEAD as additional data, so it can be read *before* any decryption — a stale file
is refused without spending a single crypto operation — and editing it fails the tag. The version
held is persisted alongside the encrypted on-disk copy, so a reboot cannot be used to reset it.

This matters because of who the attacker is. ADR-0015 D13's threat model is **whoever can write to
the bucket**. Bucket versioning is under their control; git history is on the other side of CI. Only
an in-process check helps.

A same-numbered forgery is not merely rejected, it is never opened: versions are monotonic and never
reused, so an unchanged number means an unchanged file.

## The barrier in RAM

Each snapshot generates a random AES-256-GCM key that exists only in this process, only for that
snapshot, and is never written anywhere. Every secret is sealed individually under it, and opened
into a `thread_local` scratch buffer for exactly as long as it takes to copy it into a response
body.

The subtle part, and the one that nearly made the whole thing decorative: `open()` originally
returned an owned `Contents`, so `serde_json` built a second copy of every secret as `String`/`Value`
on the heap that nothing zeroized. That copy survived in freed memory — precisely what a core dump
or a swap file picks up. `open()` now returns a borrowed view into the self-wiping buffer, and the
snapshot seals straight from those borrows. No owned copy of the cleartext is ever made.

Alongside it: `RLIMIT_CORE` set to zero, `PR_SET_DUMPABLE` cleared, and the barrier key's pages
`mlock`ed with a matching `munlock` on drop.

What this does not do is stop a live debugger, and while a response body is being written the secret
is cleartext in this process. That is what serving a secret *is*.

## Authorization without state

Vault's one genuinely stateful subsystem becomes data. The file carries a table of
`keyed-hash(token) → policy names` plus the key those hashes were computed with. There is no token
store, no lease, no expiry and no revocation endpoint: revoking is deleting a line and re-sealing.

Two details that look like inefficiencies and are not:

- The table is a **`Vec` scanned linearly** with a constant-time comparison, not a `HashMap`.
  `HashMap::get` exits early and compares strings in a way that stops at the first difference —
  which would mean the constant-time property ADR-0013 E2 asks for simply would not exist. For the
  few dozen tokens D11 sizes this at, the scan is cheaper than the HMAC that precedes it.
- The token key lives **inside the file**, not derived from the seal key. Derived, every seal-key
  rotation would silently invalidate every token in the fleet, because the operator holds the
  hashes and not the tokens, and has nothing to recompute them from.

The policy table is encrypted along with everything else, deliberately: permission to write to the
bucket and possession of the key are different things, and a policy table in the clear would let
whoever holds the first grant themselves the second.

## Observability that gets out of the way

An **access log**, not an audit log, and the distinction is the design rather than the wording. An
audit log records before it serves, so a full queue means refusing to serve. This one records after
the fact and drops when it falls behind, so a flood costs log lines instead of availability. The
count of what was dropped is a metric, so an operator can be paged on it rather than discovering the
gap later.

Every path and token is written as a keyed hash. Paths are hashed under a key *derived* from the
file's token key rather than the token key itself, because paths are chosen by the caller: under a
shared key, anyone who can request an arbitrary path and read the log would hold an oracle emitting
`HMAC(token_key, arbitrary string)` — the material for a reverse table against the file's own token
column.

Neither hash is computed per request. Path identifiers are precomputed when the file loads, since
the set of paths *is* the file's key set, and token identifiers reuse the hash the authorization
lookup already performed. An HMAC costs about what the whole RAM barrier costs; paying one per
request to write a log line would have made observing a read more expensive than serving it.

## Verification

The rule from ADR-0013 that shaped all of this: **every invariant test must be demonstrably
fail-able.** In practice that means each security test in this repository was checked by breaking
the implementation on purpose and confirming it failed — and two of them survived that check on the
first attempt and had to be rewritten, both for the same reason: they compared the implementation
against itself.

`make duck` is the project's fitness function. Three real Vault SDKs (Go, Python, PHP) against the
real server and a real bucket, plus forged files, rolled-back files, a dead bucket, a wrong key, a
revoked token and a starved log writer. It found two bugs on its first run that all 215 in-process
tests had missed, which is the whole argument for testing against clients somebody else wrote.

`docs/references/verification-status.md` records what is actually proven, what is merely believed,
and which invariants were retired along with the code they constrained.

## Build and ship

One static musl binary on a distroless base, roughly 21 MB. That was impractical while RocksDB was
in the tree; `aws-lc-rs` is now the only dependency that compiles C, and it cross-compiles cleanly.

The build needs `cmake` and `clang` for it, and `llvm` for `llvm-ar` — which is a separate package,
and a fact learned the slow way, several minutes into a build.
