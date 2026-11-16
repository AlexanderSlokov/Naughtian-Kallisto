---
title: Security model
description: The attacker Kallisto defends against, each defence and what it is worth, and the residual risks it states openly.
sidebar:
  order: 2
---

Kallisto states what each defence is worth, residuals included. This page
does the same.

## The attacker: whoever can write to the bucket

The sealed file sits in object storage that an operator, a CI job and possibly
a platform team can all reach. Object versioning and bucket policy are under
that same control, so they cannot protect you from it. Every defence around the
file aims at this attacker.

The attacker Kallisto does **not** claim to stop is root on the machine, or
anyone who can read the process's memory.

## The file

The sealed file is AES-256-GCM. Its header (magic, format version, content
version, nonce) is plaintext so the version can be read before decryption, and
it is authenticated as additional data, so editing any of it fails the tag.

Three properties follow:

- **Tampering is detected.** A changed byte anywhere fails authentication. The
  previous table stays in place and the refusal is counted.
- **Rollback is refused inside the process.** A file older than the one being
  served is rejected. Versions are monotonic and never reused. This check has to
  live in the resolver, because the bucket is under the attacker's control.
- **A restart opens no rollback window.** The resolver loads its local
  encrypted copy before it first asks the bucket, so the version it already
  trusted becomes the floor.

The policy and token tables are encrypted along with the secrets. A policy
table in the clear would let whoever holds bucket write access grant themselves
access. Bucket write permission and possession of the key stay two separate
things.

## The key

One 32-byte seal key, read from `KALLISTO_SEAL_KEY` and nowhere else. It has no
flag, because command-line arguments are readable by every process through
`ps`, and no configuration field, because the configuration file is meant to be
committed.

Everything that writes a sealed file happens offline in `kallisto-ctl`. The
server has no endpoint that writes, so there is no endpoint that can be tricked
into writing.

## The read surface

Every write route answers 403. A resolver that cannot write is a resolver whose
stolen credentials are worth nothing to an attacker who wants to plant a value.

Authorization is a token table inside the file:

- Tokens are stored as keyed hashes. The plaintext file passes through editors,
  git and CI before sealing, and keyed hashes cannot be attacked offline without
  the key, even when an operator picks a short token.
- Lookup is constant-time and compares against every entry, so timing reveals
  neither which entry matched nor whether any did.
- The permission check runs before the lookup, so a refusal reveals nothing
  about whether the secret exists.
- `deny` beats every other rule. Where this differs from Vault's
  specificity-based resolution, Kallisto refuses what Vault would allow. A 403
  is a visible, diagnosable failure. Serving a secret because a broader rule was
  worded differently is neither.
- A file with tokens and no key to check them against is refused outright, so
  authorization is never silently off.

## The port

8200 on loopback, enforced at startup. The token table controls *which secrets*
a caller may read. Nothing controls *who may reach the port*. Binding a
routable address with `--i-accept-the-risk` turns a sidecar into an
unauthenticated secrets endpoint, which is why the flag is long, has no
configuration-file form, and appears on the command line for reviewers to see.

## Memory, and what the barrier is worth

Secrets are sealed individually in RAM under a per-snapshot key that is never
written anywhere. A request opens one secret into its worker's buffer, and the
buffer is wiped before the next request. Alongside that, three best-effort
measures: core dumps off, `PR_SET_DUMPABLE` cleared so a debugger cannot
attach, and the barrier key locked into RAM so it is never swapped. Each reports
what it managed. None of them stops startup, because a resolver that refused to
run over a small `RLIMIT_MEMLOCK` in a container would trade a real outage for
a marginal gain.

**What this is worth.** An attacker who can read process memory can read the
barrier key too. The barrier removes *accidental* disclosure: a core dump, a
swapped page, a log line that printed a table. At any moment, memory holds a few
dozen ciphertexts and whichever one or two secrets are mid-response.

Mid-response is real. The HTTP body holds a secret in the clear from the moment
it is built until the socket takes it. The project calls this a barrier for
that reason. It also records two residuals: the `aws-lc-rs` prepared key types
for HMAC and AEAD hold derived key material and do not zeroize themselves.

## Logs

No secret value is ever written to either stream. Paths and tokens appear as
keyed hashes, because a few dozen guessable paths can be reversed from a bare
digest by hashing the same guesses. The log key is derived under a separate
label from the token key, so a caller who chooses paths and reads the log gains
no oracle against the token column of the file.

Error messages carry counts and positions, never content. The one deliberate
exception is the configuration parser, which quotes the offending value on a
type error, since the configuration file holds no secrets by construction.

## No audit log

An audit log records before it serves, so a full queue means refusing to
serve. Kallisto's access log records afterwards and drops when it falls behind.
Believing you can answer "who read the Stripe key" when you cannot is worse
than knowing you cannot, so the log is never called an audit log, and a test
enforces that nothing in the code is named one.

This is the main reason some secrets do not belong in Kallisto. See [use
cases](/kallisto/explanation/use-cases/).

## Verified and believed

The Kallisto repository keeps a verification ledger that separates invariants
proven by a fail-able test from those only believed. Every security-invariant
test was checked by breaking the implementation on purpose. Two survived that
check at first and were rewritten. One property, that the token lookup visits
every entry, is recorded as held by implementation and review, since no test
can distinguish an early return from a full scan.

The fuzzer targets the sealed file parser, the one input an attacker fully
controls.
