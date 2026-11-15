---
title: Monitor Kallisto
description: Scrape the metrics endpoint, alert on the signals that matter, and read the two log streams.
sidebar:
  order: 5
---

Kallisto has three telemetry surfaces, all on by default: Prometheus metrics at
`/v1/sys/metrics`, an access log on stdout, and operational messages on stderr.
There is no exporter to install.

## Scrape the metrics

```bash
curl -s http://127.0.0.1:8200/v1/sys/metrics
```

The endpoint is unauthenticated. It carries counts and a file version, and
never a path, token or value. Because the port is loopback-only, the scraper
has to run on the same machine. In Kubernetes that means a scraper inside the
pod, such as a Prometheus agent or an OpenTelemetry collector sidecar, instead
of a cluster-wide Prometheus reaching in.

Vault serves the same route, so a scrape configuration written for Vault finds
it where it expects.

## Alert on these

**Dropped access log lines.** Page on it.

```promql
increase(kallisto_access_log_dropped_total[5m]) > 0
```

The access log drops lines when its queue is full, so that a flood costs log
lines and reads keep answering. A non-zero increase means something is driving
enough load at a process that normally handles tens of thousands of reads a
second to make it shed its own bookkeeping.

**A machine drifting from the fleet.** A slower alert.

```promql
increase(kallisto_refresh_failures_total[15m]) > 0
  and changes(kallisto_file_version[15m]) == 0
```

A rising failure count with a frozen version is a machine serving an old file:
the bucket is unreachable, the file fails authentication, or it was refused as
a rollback. Expect the counter to read 1 after a first boot, since a fresh
machine has no fallback copy to warm from. Alert on it rising.

**Machines disagreeing.** Compare `kallisto_file_version` across machines
serving the same object. After a rollout they should converge within one
refresh interval.

**Sealed.** `kallisto_sealed == 1` means no file has loaded and every read
answers 503.

## Check authorization mode

```promql
kallisto_authorization_enforced == 0
```

Zero means the file carries no token table and every read is permitted. That
is correct for a single-app sidecar and a problem anywhere else. `sys/health`
reports the same thing as `"kallisto_authorization": "none"`. See [add tokens and
policies](/kallisto/how-to/add-tokens-and-policies/).

## Liveness

`GET /v1/sys/health` answers 200 with a file loaded and 503 without one, the
same two answers Vault gives for unsealed and sealed, so existing probes keep
working. A machine serving last week's file is still healthy by that
definition, which is why the version alerts above matter.

## Read the logs

- **stdout** is the access log and nothing else. One fixed-shape line per
  request, suitable for a log shipper.
- **stderr** carries operational prose: the startup banner, every refresh
  outcome, and any process hardening that could not be applied.

An access log line looks like this:

```
ts=1758537600123 seq=4812 worker=1 method=GET action=data path=3f9a0c1b2d4e5f60 token=- status=200 authz=off
```

`path` and `token` are keyed hashes, never the values themselves, and no secret
value appears on either stream. Lines from different workers can arrive out of
order, so sort by `ts` and `seq`.

:::caution[The access log is not an audit log]
An audit log records before it serves, so a full queue blocks reads. This one
records after serving and drops when it falls behind. It cannot tell you with
certainty who read what. If you need that answer, the secret belongs in a
system with a real audit log.
:::

## Turning the access log off

```yaml
spec:
  log:
    enabled: false
```

Nothing is recorded, and the server says so on stderr at startup.
`kallisto_access_log_dropped_total` stays at zero.

Every metric and the full line format are in the [metrics and logs
reference](/kallisto/references/metrics/).
