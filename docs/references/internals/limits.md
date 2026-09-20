# Limits

What Kallisto caps, what it does not, and which numbers are load-bearing.

## Rate limiting

A token bucket per worker, not per process. The configured rate is therefore *per worker*, and the
configuration field says so in its name:

```yaml
spec:
  limits:
    requestsPerSecondPerWorker: 20000
    burst: 20000
```

Both default to 20000, and `burst` defaults to whatever the rate is — one second's worth. That is
enough to absorb a thundering herd of sidecars restarting together, and not enough to hide a
runaway loop.

A process-wide limiter would put a contended atomic in front of every read and undo the whole point
of thread-per-core (ADR-0016 QĐ-3). The cost of the per-worker choice is that the effective ceiling
is `rate × workers`, and that a client pinned by `SO_REUSEPORT` to one worker sees that worker's
bucket rather than a fair share of the process.

Over the limit is answered the way Vault answers it — 429 with `Retry-After` — rather than the way
S3 does (ADR-0015 D14). `Retry-After` is rounded up and never zero, because a `Retry-After: 0` is
an invitation to hot-loop.

Internally the bucket is fixed-point, one permit being a million units, so a rate of one request
per second still refills smoothly at millisecond resolution. The atomics are `Relaxed`: a bucket
belongs to exactly one worker thread, so it is uncontended by construction and the atomics exist to
satisfy `Sync` rather than to synchronise anything.

## Size

There is no configured cap on the number of secrets. ADR-0015 D11 sized the snapshot deliberately:
a few dozen secrets, read almost exclusively, so a plain `HashMap` behind `arc-swap` is enough —
the project's own benchmark put the time in the HTTP layer, not the lookup.

The practical limits are that the whole file is held in memory twice over during a refresh (the
decrypted plaintext, briefly, and the sealed per-secret copies that replace it), and that a LIST
walks a sorted path vector by binary search rather than scanning.

An access log line is fixed at 192 bytes and allocates nothing. It cannot grow, and a test pins
that the fields at their longest still fit.

## The access log queue

```yaml
spec:
  log:
    queueCapacity: 8192
```

Floored at 2. This is the only queue in the serving path, and it drops rather than blocks when
full. Raising it buys tolerance for short bursts; it does not change the drop-on-full behaviour,
which is the design and not a limit to tune away. See
[telemetry/key-metrics.md](./telemetry/key-metrics.md) for the counter to alert on.

## Timeouts and intervals

| What | Value | Where |
|---|---|---|
| Refresh poll interval | 30 s | `refresh.intervalSeconds`, or `--refresh-interval-seconds` |
| Bucket request timeout | 15 s | fixed in `resolver/bucket.rs` |
| Workers | 2 | `workers`, or `--workers` |

The bucket timeout is why the refresh loop gets its own unpinned thread: a fifteen-second call must
never occupy a core that is answering reads.

## What is not limited

No connection cap, no request body size limit, and no per-token quota. The first two are left to
whatever runs in front of the process; the port is loopback-only, so the reachable client set is
already the set of processes on the machine.

There is no limit on *writes*, because there are none — every write route answers 403.
