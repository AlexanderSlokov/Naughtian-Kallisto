# Use Cases for Naughtian Kallisto

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

- **Yes** → Kallisto is a good fit. See
  [serving-kv-secrets.md](./serving-kv-secrets.md) for what the read path actually measures.
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
│   │  • Root CAs      │           │  • Service tokens │          │
│   │  • Master keys   │  operator │  • DB passwords   │          │
│   │  • Payment keys  │  seals a  │  • API keys       │          │
│   │  • PII keys      │  file ──► │  • Session keys   │          │
│   │                  │  (bucket) │  • TLS certs      │          │
│   │  Rare reads      │           │  Read-path hot    │          │
│   │  Full audit      │           │  No audit log     │          │
│   └──────────────────┘           └───────────────────┘          │
│         ▲                               ▲                       │
│         │ Rare (admin, rotation)        │ Frequent (every req)  │
│         │                               │                       │
│   ┌─────┴───────────────────────────────┴─────┐                 │
│   │            Your Microservices             │                 │
│   └───────────────────────────────────────────┘                 │
└─────────────────────────────────────────────────────────────────┘
```

There is no automatic sync between the two. An operator exports the operational secrets, seals
them into one file with `kallisto-ctl` (AES-256-GCM, offline, never over the network), and puts
that file on a bucket; each machine's Kallisto polls it. Envelope encryption through a Transit
engine, a KEK/DEK hierarchy and BoringSSL were all part of the secrets-server design that
ADR-0015 deleted — Kallisto now holds one key, taken from the environment at startup.

Your services read from Kallisto on localhost. If a machine is compromised, you rotate the seal
key and re-seal; the file's content version is authenticated, and a version older than the one
a server already holds is refused.

The mechanics are in [what-is-kallisto.md](../what-is-kallisto.md) and
[architecture.md](../architecture.md).