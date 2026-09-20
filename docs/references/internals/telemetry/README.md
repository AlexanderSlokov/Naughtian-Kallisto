# Telemetry

Three things: an access log on stdout, an error log on stderr, and counters scraped from
`/v1/sys/metrics`. They live in `components/kallisto_telemetry`.

- [enable-telemetry.md](./enable-telemetry.md) — turning it on, and what the knobs do.
- [key-metrics.md](./key-metrics.md) — every metric, and which one to alert on.

## It is not an audit log

This is the whole design, not a wording preference, and ADR-0015 D15 is blunt about it. An audit
log records *before* it serves, so a full queue means refusing to serve. This one records after the
fact and drops when it falls behind, so a flood costs log lines rather than availability.

Kallisto has the second kind. Calling it the first would let somebody believe they can answer "who
read the Stripe key" when they cannot, and a limit you have hidden is a trap. A test in
`tests/security_invariants.rs` enforces that nothing in the crate is named one, rather than trusting
the convention to hold.

What is lost is counted, not silently discarded — see `kallisto_access_log_dropped_total` in
[key-metrics.md](./key-metrics.md).

## Nothing identifying is written in the clear

Paths and tokens go through a keyed hash before they reach a line; secret values never appear at
all. This applies to the error log exactly as much as the access log, because a path leaking
through a stack trace has leaked just as badly.

The hash is *keyed* rather than a bare digest for a concrete reason: a deployment has a few dozen
paths and they are guessable — `app/db`, `prod/stripe`. A bare SHA-256 over that set is reversed by
computing the same few dozen digests. A key the attacker does not hold removes that.

The key is derived from the sealed file's own token key under a separate label
(`kallisto/log/v1`), so identifiers mean the same thing across a restart and an operator holding
the file can work out which path a line refers to. The separate label matters: paths are chosen by
the caller, so a shared label would hand anyone who can send a request and read the log an oracle
producing `HMAC(token_key, arbitrary string)` — exactly the material for a reverse table against
the token column of the file.

A file with no token table has no key to derive from, so the process generates a random one that
lasts only as long as the process. Identifiers then correlate within one process lifetime and not
beyond it, and the server says so at startup rather than letting an operator assume otherwise.

## Per worker, summed at scrape time

Counters belong to one worker each and are added up only when somebody scrapes. A single shared
counter would be a contended cache line on the read path, which is what ADR-0016 QĐ-3 rearranged
the whole server to avoid. The same is true of the log: each worker has its own producer, and the
only thing two workers share is the queue itself.

One consequence worth knowing when reading a log: arrival order at the writer is not creation
order. Every line therefore carries its own sequence number and a timestamp taken when the request
happened, not when the writer got to it.
