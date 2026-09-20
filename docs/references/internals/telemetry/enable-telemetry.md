# Enabling telemetry

All three surfaces are on by default. There is nothing to install and no exporter to run: the
metrics endpoint is part of the read surface, and the logs are stdout and stderr.

## Configuration

```yaml
spec:
  log:
    queueCapacity: 8192
    enabled: true
```

`enabled` defaults to true. Setting it false records nothing at all — see the note in
[key-metrics.md](./key-metrics.md) about why that is not the same as draining nothing. The server
writes a line to the error log at startup when the access log is off, so a silent log is never
mistaken for a quiet one.

`queueCapacity` is the number of lines buffered between the workers and the writer thread, floored
at 2. Raising it buys tolerance for short bursts at the cost of memory; it does not change the
drop-on-full behaviour, which is the design rather than a limit to tune away.

## Scraping

```
GET /v1/sys/metrics
```

No authentication — it is on the loopback-only port, like everything else, and it carries no secret
values or paths in the clear. A Prometheus job pointed at `127.0.0.1:8200` needs no other setup.

Because the port is loopback, the scraper has to run on the same machine. In Kubernetes that means
scraping from within the pod rather than from a cluster-wide Prometheus reaching in.

## Reading the logs

stdout is the access log and nothing else. stderr carries operational messages, including the
startup banner, every refresh outcome, and any hardening that could not be applied. Both observe
the same hygiene: keyed identifiers, never a secret value, never a path in the clear.

A log shipper should therefore treat the two streams differently — one is structured lines of a
single fixed shape, the other is prose.

## What to alert on

One metric: `kallisto_access_log_dropped_total`. See [key-metrics.md](./key-metrics.md) for why,
and for the rest of what is exposed.

Also worth a slower alert: `kallisto_refresh_failures_total` rising while `kallisto_file_version`
stays put, which is a machine quietly serving a file older than the fleet's.
