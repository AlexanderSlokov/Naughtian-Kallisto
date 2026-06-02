---
title: "Use Cases for Naughtian Kallisto"
weight: 2
---

Kallisto is mainly designed for storing **operational secrets**: secrets that your services need at high frequency and low latency, but whose blast radius is limited and recoverable through revocation. If a secret's leak would trigger a compliance incident, a regulatory investigation, or irreversible financial damage - it belongs in Vault/OpenBao, not here.

### Good Fit for Kallisto

| Secret Type                                      | Why it fits                                     | Example                                     |
|--------------------------------------------------|-------------------------------------------------|---------------------------------------------|
| **Internal service-to-service tokens**           | High read rate, short-lived, easily revoked     | gRPC auth tokens between microservices      |
| **Database connection strings** (non-production) | Rotated frequently, scoped to dev/staging       | `postgres://app:pass@staging-db:5432/myapp` |
| **Feature flag encryption keys**                 | Read on every request, low sensitivity          | Keys for encrypting A/B test configs        |
| **Session signing keys**                         | Read-heavy (~99/1 R/W), rotatable               | JWT HMAC keys for internal dashboards       |
| **Cache authentication**                         | Sub-millisecond reads needed, revocable         | Redis AUTH passwords for internal caches    |
| **CI/CD pipeline tokens**                        | Bursty reads during deployments, short TTL      | Temporary deploy tokens for Kubernetes      |
| **Internal API keys**                            | High-throughput reads, easily regenerated       | API keys for internal observability tools   |
| **TLS certificates for internal mTLS**           | Read at connection setup, rotated by automation | Intermediate CAs for service mesh           |
| **Configuration encryption keys**                | Read-dominant, app-scoped                       | Keys for encrypting config files at rest    |

### Do NOT Store in Kallisto

| Secret Type                                            | Why it doesn't fit                      | Where it belongs                         |
|--------------------------------------------------------|-----------------------------------------|------------------------------------------|
| **Root CA private keys**                               | Catastrophic if leaked, rarely accessed | HSM / Vault with HSM backend             |
| **Payment processor secret keys** (`Stripe sk_live_*`) | Direct financial damage, PCI-DSS scope  | Vault with audit + compliance policies   |
| **Cloud provider root credentials** (AWS root, GCP SA) | Full account takeover, irrecoverable    | Vault + MFA + break-glass procedure      |
| **Customer PII encryption master keys**                | GDPR/CCPA scope, regulatory liability   | Vault with FIPS 140-2 backend            |
| **SSH keys to production bastions**                    | Direct infrastructure access            | Vault SSH secrets engine or signed certs |
| **Signing keys for software releases**                 | Supply chain attack vector              | Air-gapped HSM                           |

### The Decision Rule

**Ask yourself:** *If this secret leaks and I revoke it within 5 minutes, is the damage contained and recoverable?*

- **Yes** → Kallisto is a great fit. You get 1M+ RPS reads and sub-millisecond p99 latency.
- **No** → Use Vault/OpenBao with full audit trails, compliance policies, and HSM integration.

### Recommended Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                     Your Infrastructure                         │
│                                                                 │
│   ┌──────────────────┐           ┌───────────────────┐          │
│   │  Vault / OpenBao │           │     Kallisto      │          │
│   │  (Root of Trust) │           │ (Operational KV)  │          │
│   │                  │           │                   │          │
│   │  • Root CAs      │──[sync]──▶ • Service tokens   │          │
│   │  • Master keys   │           │  • DB passwords   │          │
│   │  • Payment keys  │           │  • API keys       │          │
│   │  • PII keys      │           │  • Session keys   │          │
│   │                  │           │  • TLS certs      │          │
│   │  ~500 RPS        │           │  ~36k RPS/core    │          │
│   │  Full audit      │           │  Low latency      │          │
│   └──────────────────┘           └───────────────────┘          │
│         ▲                               ▲                       │
│         │ Rare (admin, rotation)        │ Frequent (every req)  │
│         │                               │                       │
│   ┌─────┴───────────────────────────────┴─────┐                 │
│   │            Your Microservices             │                 │
│   └───────────────────────────────────────────┘                 │
└─────────────────────────────────────────────────────────────────┘
```

Each Root of Trust can have its own **Transit Engine** (envelope encryption). It holds the Master Key and wraps/unwraps Kallisto's KEK (Key Encryption Key) at startup. Kallisto uses the KEK to encrypt/decrypt DEKs locally, which BoringSSL uses for AES-256-GCM encryption at rest. Your services read from Kallisto at wire speed. If Kallisto is compromised, you revoke all derived keys from the Root of Trust and the blast radius is contained.