---
title: Use cases
description: The problems Kallisto 2.0.0 solves, which secrets belong in it, and when it is the wrong tool.
sidebar:
  order: 3
---

Kallisto holds **operational secrets**: values your services need often and
fast, whose leak is contained by revoking them. The secrets whose leak would
start a compliance incident or cause irreversible damage stay in Vault or
OpenBao.

## The decision rule

> If this secret leaks and I revoke it within five minutes, is the damage
> contained and recoverable?

**Yes:** a good fit for Kallisto. **No:** keep it in Vault or OpenBao, with
full audit trails, compliance policy and HSM integration.

## What it solves

### Reads that must survive an unreachable upstream

A central secret store has windows where it serves nothing. Vault is sealed
after every restart, a Raft failover means a leader election, and an upgrade
means a careful step-down. A failed secret read rarely degrades gracefully: a
process that cannot fetch its credentials usually does not start at all.

With Kallisto, a request depends on a process on the same machine and a file it
already holds. The secret store that produced the file can be down. The bucket
can be down too: each resolver keeps serving the last authenticated file, and
restarts from its encrypted local copy. A hard runtime dependency on a
quorum-bound system becomes a periodic, offline export.

### Per-request secret access

Reading a secret right before use and discarding it right after keeps
plaintext out of application memory for as long as possible, and makes a
rotation take effect without a restart. Against a central Vault that costs a
network round trip per use. Against Kallisto it is a loopback call answered
from RAM, at latencies around a millisecond. See [performance and
footprint](/kallisto/references/performance/).

### Edge and unreliable links

Gateways and edge nodes need current certificates and keys but often sit on
links to the core that drop. A resolver per node keeps serving what it has
while the link is down, and picks up the next version when it returns. This
composes with [Helvilette](/helvilette/), which targets the same fleets for the
same reason.

### Certificates for mTLS

A service mesh needs a leaf certificate and its private key on every workload.
An intermediate CA or the OpenBao PKI engine issues the leaves, an operator
seals the current set into one file, and each workload reads its own pair from
`127.0.0.1:8200` with its Vault client. When a deployment restarts, its pods
read their keys within the same second. The burst of each worker's rate limit,
one second of requests by default, absorbs that peak.

Kallisto issues, signs and renews nothing, and it does not read the expiry of a
certificate. It serves an expired leaf like any other value until the file is
sealed again. Compare `kallisto_file_version` across machines to see which
generation each one serves. Root CA private keys stay in an HSM.

### Removing `.env` files

A sealed file on a bucket, plus a key in the orchestrator's secret store,
replaces plaintext `.env` files on disk, in backups and on laptops. The
application reads through the Vault API it may already speak.

## Good fits

| Secret | Why it fits |
|---|---|
| Internal service-to-service tokens | High read rate, easily revoked |
| Database passwords for non-production | Rotated often, limited scope |
| Session and JWT signing keys for internal apps | Read-heavy, rotatable |
| Cache authentication (Redis `AUTH`) | Sub-millisecond reads wanted, revocable |
| Internal API keys | High throughput, easily regenerated |
| TLS certificates and keys for internal mTLS | Read at connection setup, rotated by automation |
| Configuration encryption keys | Read-dominant, app-scoped |

## Keep these out

| Secret | Why | Where it belongs |
|---|---|---|
| Root CA private keys | Catastrophic if leaked | HSM, or Vault with an HSM backend |
| Live payment keys (`sk_live_...`) | Direct financial loss, PCI-DSS scope | Vault with audit and compliance policy |
| Cloud root credentials | Full account takeover | Vault with MFA and break-glass procedure |
| PII encryption master keys | Regulatory liability | Vault with a FIPS-validated backend |
| SSH keys to production bastions | Direct infrastructure access | Vault SSH engine or signed certificates |
| Release signing keys | Supply-chain attack vector | Air-gapped HSM |

The common reason is the audit log Kallisto does not have. Its access log drops
lines under load, so it cannot prove who read a secret.

## How it fits next to Vault

```
┌──────────────────┐                        ┌───────────────────┐
│  Vault / OpenBao │   operator exports,    │     Kallisto      │
│  root of trust   │   seals with           │  one per machine  │
│                  │   kallisto-ctl, and    │                   │
│  root CAs        │   uploads  ─────────►  │  service tokens   │
│  master keys     │   (bucket)             │  DB passwords     │
│  payment keys    │                        │  API keys, certs  │
│  rare reads      │                        │  every request    │
│  full audit      │                        │  no audit log     │
└──────────────────┘                        └───────────────────┘
          ▲  admin, rotation                          ▲  loopback reads
          └──────────────── your services ────────────┘
```

Nothing syncs the two automatically. An operator or a CI job exports the
operational secrets, seals them into a file with `kallisto-ctl`, and uploads it.
Each machine's Kallisto polls that file.

## When Kallisto is the wrong tool

- **You need dynamic secrets, leases, PKI or transit.** Kallisto serves static
  values from a file.
- **You need an audit trail of reads.** The access log is best-effort.
- **You need something production-grade today.** 2.0.0 is a prototype. See
  [project status](/kallisto/references/status/).
- **Callers are on other machines.** Kallisto serves loopback only and has no
  network authentication. Run one per machine.
- **Your reads are rare and Vault is comfortable.** A handful of reads at boot
  against a healthy Vault gains little from another component.
