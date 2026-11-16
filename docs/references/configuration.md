---
title: Configuration
description: Every kallisto-server setting, with its YAML field, environment variable, flag and default.
sidebar:
  order: 1
---

`kallisto-server` reads a Kubernetes-shaped YAML file. The file holds no
secret and has nowhere to put one: the seal key and bucket credentials come
from the environment only.

## Precedence

For every setting, the strongest source wins:

**command-line flag > environment variable > YAML file > built-in default**

Environment variables set to an empty string count as unset.

## Minimal file

```yaml
apiVersion: kallisto/v1
kind: Resolver
spec:
  source:
    type: disk
    path: /etc/kallisto/secrets.kal
```

Everything else has a default. `apiVersion` must be `kallisto/v1` and `kind`
must be `Resolver`. Unknown fields fail to parse, so a typo is an error and
never a silently ignored line. Check a file with
`kallisto-ctl validate --config <file>`.

## Settings

| YAML field (under `spec`) | Environment variable | Flag | Default |
|---|---|---|---|
| (the file itself) | `KALLISTO_CONFIG` | `--config` | required |
| `listen.address` | `KALLISTO_LISTEN_ADDRESS` | `--listen-address` | `127.0.0.1` |
| `listen.port` | `KALLISTO_LISTEN_PORT` | `--listen-port` | `8200` |
| `workers` | `KALLISTO_WORKERS` | `--workers` | `2` |
| `mount` | | | `secret` |
| `source` | | | required |
| `refresh.intervalSeconds` | `KALLISTO_REFRESH_INTERVAL_SECONDS` | `--refresh-interval-seconds` | `30` |
| `cacheDir` | `KALLISTO_CACHE_DIR` | `--cache-dir` | none |
| `limits.requestsPerSecondPerWorker` | | | `20000` |
| `limits.burst` | | | same as the rate |
| `log.enabled` | | | `true` |
| `log.queueCapacity` | | | `8192`, floored at 2 |
| | | `--i-accept-the-risk` | off |

Flags accept both `--flag value` and `--flag=value`. `-h` or `--help` prints
the usage.

### `listen`

The address and port to serve on. Any non-loopback address is refused at
startup unless `--i-accept-the-risk` is also given. IPv6 loopback, `::1`, counts
as loopback. The flag has no YAML or environment form, so accepting the risk
always shows on the command line where a reviewer can see it.

### `workers`

Serving threads. Each is a single-threaded Tokio runtime pinned to its own
core, and all of them share the port through `SO_REUSEPORT`. Must be at least 1.
Use `1` under a low container CPU limit, and raise it towards the core count on
larger machines.

### `mount`

The KV-v2 mount path applications already use. With `secret`, reads go to
`/v1/secret/data/<path>`.

### `source`

Where the sealed file comes from. Exactly one of:

```yaml
source:
  type: bucket
  endpoint: https://s3.example.com   # required
  bucket: kallisto                   # required
  objectKey: prod/payment.kal        # required
  region: auto                       # default: auto
  pathStyle: true                    # default: false
```

```yaml
source:
  type: disk
  path: /etc/kallisto/secrets.kal    # required
```

See [configure the source](/kallisto/how-to/configure-the-source/).

### `refresh.intervalSeconds`

How often the source is checked. A bucket poll sends the last ETag, so an
unchanged object costs a `304`.

### `cacheDir`

Directory for the encrypted fallback copy, written as `secrets.kal` inside it.
With no directory set there is no copy, and a machine restarted while the
source is unreachable stays sealed.

### `limits`

A token bucket **per worker**. The effective ceiling for the process is
`requestsPerSecondPerWorker × workers`. Over the limit, requests get
`429 Too Many Requests` with a `Retry-After` header, rounded up and never zero.
The kernel keeps each connection on one worker, so a client with one connection
gets the limit of one worker.

### `log`

`enabled` switches the access log on stdout. `queueCapacity` is the number of
lines buffered between the workers and the writer thread. When the queue is
full, lines are dropped and counted in `kallisto_access_log_dropped_total`.
Raising the capacity absorbs longer bursts. Lines are still dropped once the
queue fills.

### Fixed limits

The following limits have no setting:

- A request to the bucket times out after 15 seconds. The refresh loop runs on
  its own thread, so a slow bucket does not delay reads.
- A file can hold any number of secrets. During a refresh the process holds the
  file twice: the decrypted plaintext for a short time, and the sealed copy of
  each secret that replaces it.
- Kallisto does not limit connections, request body size, or requests per
  token. The port is on loopback, so only processes on the same machine reach
  it.

## Secrets from the environment

| Variable | Purpose | Required |
|---|---|---|
| `KALLISTO_SEAL_KEY` | 32-byte seal key, hex-encoded | Always |
| `KALLISTO_S3_ACCESS_KEY_ID` | Bucket access key | For `type: bucket` |
| `KALLISTO_S3_SECRET_ACCESS_KEY` | Bucket secret key | For `type: bucket` |

None of these can be passed as a flag or written in the file. `--seal-key` is
recognised only so that it can be refused with an explanation: command-line
arguments are visible to every process on the host through `ps`.

## Full example

The Kallisto repository ships `kallisto.example.yaml`, commented field by
field:

```yaml
apiVersion: kallisto/v1
kind: Resolver
spec:
  listen:
    address: 127.0.0.1
    port: 8200
  workers: 2
  mount: secret
  source:
    type: bucket
    endpoint: https://s3.example.com
    bucket: kallisto
    objectKey: prod/payment.kal
    region: auto
    pathStyle: true
  refresh:
    intervalSeconds: 30
  cacheDir: /var/lib/kallisto
  limits:
    requestsPerSecondPerWorker: 20000
    burst: 20000
  log:
    queueCapacity: 8192
    enabled: true
```

Tokens and policies are absent here on purpose. They live inside the sealed
file. See [the sealed file](/kallisto/references/sealed-file/).
