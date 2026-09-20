# Key metrics

Scraped from `GET /v1/sys/metrics`, Prometheus text exposition format 0.0.4. Every metric carries
its `HELP` and `TYPE`; a test asserts that, because a metric a scraper treats as untyped is a
metric nobody can alert on.

## The one to alert on

```
kallisto_access_log_dropped_total
```

Access log lines discarded because the queue was full. A non-zero value means the daemon is
shedding its own bookkeeping to keep serving secrets. That is the right trade — ADR-0015 D15 chose
it deliberately — but it is also exactly what somebody should be woken for, because something is
driving enough load at a process that comfortably handles tens of thousands of reads a second.

It is present at zero, always. A metric that only appears once it is broken cannot have an alert
written against it (QĐ-8).

Note the interaction with `log.enabled: false`: switching the log off records nothing rather than
enqueuing into a queue no writer drains. The second shape would fill up and then count every line
as dropped, raising this alarm for the one reason that is not an incident.

## Requests

```
kallisto_requests_total{outcome="ok|denied|not_found|rate_limited|sealed|error"}
```

Six labels, mapped from the status code: 2xx is `ok`, 403 `denied`, 404 `not_found`, 429
`rate_limited`, 503 `sealed`, anything else `error`. A label per distinct status code would be
unbounded cardinality; these are the answers this server actually gives.

Summed across workers at scrape time. A scrape lands on whichever worker `SO_REUSEPORT` handed the
connection to, so every worker can read every counter while only ever writing its own.

## The file being served

```
kallisto_sealed                     1 when no file has loaded, so every read answers 503
kallisto_file_version               content version of the file being served (0 when sealed)
kallisto_secrets                    secrets in that file
kallisto_tokens                     tokens in that file
kallisto_authorization_enforced     0 when the file carries no token table
```

`kallisto_authorization_enforced` at 0 is the sidecar deployment of ADR-0015 D8: no token table
means every read is permitted, and the bucket credential was the boundary. That is legitimate, and
it is a gauge so that it is visible rather than discovered.

`kallisto_file_version` across several machines answers "do these three agree", which is what the
content version exists for.

## Refresh health

```
kallisto_refresh_failures_total
```

Polls that did not yield a servable file — a rejected file, a failed authentication, an
anti-rollback refusal, or a bucket that did not answer. Increments on every failed poll, not just
the first.

This is how "this machine has been serving a stale file for an hour" becomes something a scrape
notices rather than something somebody reads the logs to discover. Pair it with
`kallisto_file_version`: a rising failure count with a frozen version is a machine drifting away
from the fleet.

## Access log line format

One line per request, on stdout, rendered at creation:

```
ts=<millis> seq=<n> worker=<n> method=<M> action=<A> path=<id> token=<id> status=<code> authz=on|off
```

`action` is drawn from a closed set — `data`, `metadata`, `list`, `sys`, or `-` — never from the
URI, so no caller-supplied text can reach a line unhashed. `path` and `token` are 16 hex characters
of a keyed HMAC-SHA256; see [README.md](./README.md) for which key and why.

Lines are fixed-capacity and allocate nothing. stdout carries the access log and nothing else —
operational messages go to stderr — so a shipper can read it as a stream of one shape.
