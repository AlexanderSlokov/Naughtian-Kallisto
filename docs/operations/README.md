# Operations

Building and developing Kallisto:

- [development/toolchain-setup.md](./development/toolchain-setup.md) — read this before your first
  build. It covers the pinned nightly toolchain, local CI reproduction, and the `Makefile` targets.

Deployment and configuration guides for operators live on
[docs.naughtian.org/kallisto](https://docs.naughtian.org/kallisto/), not here.

The `audit/` and `deploy/kubernetes/csi/` sections that used to be listed here were empty
placeholders for features that do not exist: Kallisto has no audit log (see ADR-0015 D15 — the
access log drops on a full queue, so calling it one would be a trap) and no CSI driver.
