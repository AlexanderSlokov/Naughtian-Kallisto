# Internals

How the pieces work, for someone changing them. The shape of the whole system is in
[architecture.md](../../explanation/architecture.md); these pages go a layer down.

- [token.md](./token.md) — the token table, constant-time lookup, and how Vault's policy path
  syntax is reproduced including the KV-v2 `data/` quirk.
- [limits.md](./limits.md) — the rate limiter, the queue, the timeouts, and what is deliberately
  not limited.

The operator's side of these topics is in the pages that come from the site:
[configuration.md](../configuration.md) gives every limit, [metrics.md](../metrics.md) gives the
metrics and the access log, and
[add-tokens-and-policies.md](../../how-to/add-tokens-and-policies.md) gives the token workflow.

The decisions behind all of this are in [ADRs/](../ADRs/); ADR-0015 and ADR-0016 are the two that
produced the current system. What is actually proven versus merely believed is in
[verification-status.md](../verification-status.md).
