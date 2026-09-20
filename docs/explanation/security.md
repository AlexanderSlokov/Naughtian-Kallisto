# Security

What Kallisto defends against, what it does not, and what each defence is actually worth. The
project's habit is to state the residual rather than round it away, so this page does the same.

## The threat model

The attacker this design takes seriously is **whoever can write to the bucket**.

That follows from the deployment: the file sits in object storage that an operator, a CI job, and
possibly a platform team can all reach. Object versioning and bucket policy are under that same
control, so they cannot be the thing that protects you from it. Everything below that looks
defensive about the file is aimed here.

The attacker it does *not* claim to stop is **root on the machine**, or anyone who can read the
process's memory. ADR-0015 D13 says this in as many words, and the code repeats it where the
defences live.

## The file

The sealed file is AES-256-GCM. Its header — magic, format version, content version, nonce — is
plaintext so the version can be read before anything is decrypted, and it is passed as additional
authenticated data, so editing any of it fails authentication rather than producing a different
plaintext.

Three properties follow:

**Tampering is detected, not merely unlikely.** A modified byte anywhere fails the tag check and
the file is refused. The previous snapshot stays exactly where it was, and the refusal is counted.

**Rollback is refused in-process.** A file whose content version is older than the one already
being served is rejected. Versions are monotonic and never reused. This check cannot live in the
bucket, for the reason above.

**A restart is not a rollback window.** The resolver warms from its local encrypted copy before it
first asks the bucket, so the version it already trusted becomes the floor.

The policy and token tables are encrypted with everything else. A policy table in the clear would
let whoever holds bucket write access grant themselves the key.

## The key

One key, 32 bytes, read from `KALLISTO_SEAL_KEY` and nowhere else. Not a flag, because command-line
arguments are readable by every process on the host through `ps`; not a configuration field,
because that file is meant to be committed. `--seal-key` exists only to refuse it with that
explanation.

Everything that *writes* a sealed file happens offline in `kallisto-ctl`, never over the network.
There is no endpoint that can be tricked into writing, because there is no endpoint that writes.

## The read surface

Every write route answers 403. `subkeys`, `delete`, `undelete` and `destroy` are routed explicitly
so they answer 403 rather than 404 — a 404 reads as "not deployed yet", where 403 says the door
exists and is shut.

This is the product, not an unimplemented feature. A resolver that cannot write is a resolver whose
credentials are worth nothing to an attacker who steals them.

Authorization is a token table inside the file, looked up in constant time, with Vault's policy
path syntax reproduced exactly. Default deny, including for a request that carried no token. Where
Kallisto and Vault disagree on a contradictory policy, Kallisto refuses what Vault would have
allowed. Details in [references/internals/token.md](../references/internals/token.md).

A permission check runs *before* the lookup, so a refusal says nothing about whether the secret
exists.

## The port

8200, loopback only, refused otherwise at startup unless the operator passes
`--i-accept-the-risk`. Kallisto has no network authentication story: the token table decides which
secrets a caller may read, not who may reach the port. Binding a routable address turns a sidecar
into an unauthenticated secrets endpoint.

Overload is a 429 with `Retry-After`, from a bucket per worker.

## Memory, and what the barrier is worth

Secrets are sealed individually in RAM under a key that exists only in this process, only for the
life of one snapshot, and is never written anywhere. A request opens exactly the one secret it
asked for, into a buffer belonging to that worker thread, and the buffer is wiped before the next
request. A new file means a new key; the old one is zeroed when the old snapshot drops.

Alongside it, three best-effort measures: core dumps disabled, `PR_SET_DUMPABLE` cleared so a
debugger cannot attach, and the barrier key locked into RAM so it is not written to swap. Each
reports what it managed; none of them refuses to start, because a resolver that died over a 64 KB
`RLIMIT_MEMLOCK` in somebody's container would be trading a real outage for a marginal hardening.

**What this is worth, stated plainly.** It is not a wall against root. An attacker who can read
process memory can read the barrier key sitting in it. What it removes is *accidental* disclosure —
a core dump, a swapped-out page, a log line that printed a whole table — by making the answer to
"what is in this process's memory right now" be "a few dozen ciphertexts, and whichever one or two
secrets are mid-flight".

And mid-flight is real. The HTTP response body holds a secret in the clear from the moment it is
built until the socket has taken it, and nothing here changes that. It is called a barrier and not
protection for that reason.

Two residuals are recorded rather than hidden: `aws_lc_rs`'s prepared HMAC and AEAD key types hold
derived key-equivalent material and do not zeroize themselves. This is why D13 is written as making
a memory dump *less rewarding* rather than as a guarantee.

## Logs

No secret value is ever written, to either stream. Paths and tokens appear as keyed hashes — keyed
rather than digested, because a deployment has a few dozen guessable paths and a bare SHA-256 over
that set is reversed by computing the same few dozen digests. The log derives its own key under a
separate label from the token key's, so that a caller who can choose paths and read the log does
not hold an oracle against the token column of the file.

The error log is held to the same standard, because a path leaking through a stack trace has leaked
just as badly. Error types in this workspace carry counts and positions rather than content, and
`tests/security_invariants.rs` holds them to it.

**There is no audit log**, and the distinction is load-bearing rather than pedantic: an audit log
records before it serves, so a full queue means refusing to serve; this one records afterwards and
drops. Believing you can answer "who read the Stripe key" when you cannot is worse than knowing you
cannot. A test enforces that nothing in the telemetry crate is named one.

## What is verified, and what is only believed

[references/verification-status.md](../references/verification-status.md) is the honest ledger. It
distinguishes invariants proven by a test that was checked by deliberately breaking the
implementation, from invariants that are merely believed, from groups retired when the code they
described was deleted.

Two examples of the standard it holds: every security-invariant test in this repository was checked
by breaking the implementation and confirming the test failed — two survived that check on the
first attempt and were rewritten. And the constant-time token loop's final property, that it visits
every entry rather than returning at the first match, is recorded as *not* provable by test rather
than quietly claimed.

`fuzz/sealed_file` targets the one input an attacker fully controls.

## Reporting something

This is a prototype under active rework and is not production-ready. If you find a flaw, raise it
on the repository rather than against a deployment.
