---
title: Project status
description: What 2.0.0 implements, what it deliberately leaves out, what it does not protect against, and the licence.
sidebar:
  order: 7
---

Kallisto 2.0.0 is a **prototype under active rework**. The project's own
advice is not to run it where it matters yet, and it makes no stability
promise. This page collects the caveats in one place.

## What 2.0.0 is

2.0.0 replaced the secrets server of the 1.x line with a local, read-only
resolver. The design is set out in ADR-0015 and amended by ADR-0016 in the
Kallisto repository.

## Implemented

| Component | State |
|---|---|
| Vault KV-v2 read surface (`data`, `metadata`, LIST) | Working, tested with three real SDKs |
| Write routes | Answer 403 by design |
| Bucket source (S3-compatible, SigV4, ETag polling) | Working |
| Disk source | Working |
| AES-256-GCM sealed file with authenticated header | Working |
| Anti-rollback on content version, including across restarts | Working |
| Encrypted local fallback copy | Working |
| Token table and Vault-syntax policies | Working |
| Per-secret encryption in RAM between requests | Working |
| Process hardening (no core dumps, no ptrace attach, key locked in RAM) | Best effort, reported at startup |
| Loopback-only bind, enforced at startup | Working |
| Per-worker rate limiting with 429 and `Retry-After` | Working |
| Prometheus metrics and access log | Working |
| `kallisto-ctl` offline tool | Working |

## Removed in 2.0.0

The following existed in the 1.x secrets server and were deleted, because the
problem the project solves changed:

- The KV-v2 write path, version history, CAS, soft delete and destroy.
- The storage engine: RocksDB, the sharded cuckoo cache, CLOCK eviction.
- The admin server on port 8202 and the gossip controlplane.
- The C++ core and its FFI bridge.

Plans for `redb`, Raft, leases, proxy mode and multi-node operation are
superseded along with them.

## Out of scope by design

Auth methods, dynamic secrets, leases, token expiry, PKI, transit and any other
secret engine. Kallisto is a resolver that sits beside a root of trust. For
those features, run OpenBao or Vault.

## What it does not protect against

- **Root on the machine, or anyone who can read the process's memory.** The
  in-RAM barrier makes a core dump or a swap file less rewarding. A live
  debugger with root still wins. Nothing at this layer can stop that.
- **A response in flight.** While a secret is written into an HTTP response, it
  is cleartext in the process. Serving a secret means exactly that.
- **Anyone holding the seal key.** They can read the file. Keep the key in your
  orchestrator's secret store.
- **Network callers, if the port is exposed.** There is no network
  authentication. Binding beyond loopback with `--i-accept-the-risk` makes
  Kallisto an unauthenticated secrets endpoint for everyone who can reach it.
- **Questions an audit log answers.** The access log drops lines under load.
  It cannot say with certainty who read what.

## Verification

The Kallisto repository keeps a ledger at
`docs/references/verification-status.md` that separates what is proven by a
test, what is only believed, and what was retired with deleted code. Every
security-invariant test was checked by deliberately breaking the
implementation and confirming the test failed. `make verify` is the blocking
gate. Loom, fuzzing and mutation testing run on a schedule.

## Licensing

Kallisto is **AGPLv3** (`AGPL-3.0-or-later`). A commercial licence can be
discussed with the author.

For the wider ecosystem picture, see [Kuberina's licensing
reference](/kuberina/references/licensing/).

## Documentation

This site holds the operator documentation: tutorials, how-to guides and
reference. The Kallisto repository's `docs/` directory holds the engineering
records, as plain markdown read on GitHub:

- **ADRs** under `docs/references/ADRs/`. ADR-0015 and ADR-0016 define the
  current design. Superseded ADRs say so in their front matter.
- **Verification status**, described above.
- **Benchmarks** under `docs/references/benchmarks/`. Most reports predate
  2.0.0 and measure the deleted write path.
- **Architecture in depth**, including the two abandoned architectures, under
  `docs/explanation/how-to-create-naughtian-kallisto/`.
