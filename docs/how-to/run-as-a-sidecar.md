---
title: Run Kallisto as a sidecar
description: Deploy the resolver next to an application with Docker Compose or in a Kubernetes pod, keeping port 8200 on loopback.
sidebar:
  order: 4
---

Kallisto is built to run beside the application that reads from it: one
resolver per machine, VM or pod. Whatever the layout, two rules hold:

1. **Port 8200 stays on loopback.** Kallisto has no network authentication.
   The token table decides *which secrets* a caller may read. Nothing decides
   *who may reach the port*, so reachability has to be limited by where the
   port listens.
2. **The seal key and bucket credentials come from the environment.** They
   have no field in the configuration file, which is meant to be committed.

The image is `ghcr.io/alexanderslokov/naughtian-kallisto`. It is distroless,
runs as uid 65532, and contains `kallisto-server` (the entrypoint) and
`kallisto-ctl`.

## Docker Compose

This layout is based on the Kallisto repository's `docker-compose.yml`, with
one addition: the `command` line, explained below.

```yaml
services:
  # Named volumes are created owned by root. The image runs as uid 65532 and
  # has no shell to fix that, so a one-shot container does it.
  cache-owner:
    image: alpine:3.20
    command: chown -R 65532:65532 /var/lib/kallisto
    volumes:
      - kallisto-cache:/var/lib/kallisto
    user: "0:0"

  kallisto:
    image: ghcr.io/alexanderslokov/naughtian-kallisto:latest
    depends_on:
      cache-owner:
        condition: service_completed_successfully
    # Bind every interface inside the container, so the published port reaches it.
    command: ["--listen-address=0.0.0.0", "--i-accept-the-risk"]
    ports:
      - "127.0.0.1:8200:8200"
    volumes:
      - ./kallisto.yaml:/etc/kallisto/kallisto.yaml:ro
      - kallisto-cache:/var/lib/kallisto
    environment:
      KALLISTO_SEAL_KEY: "${KALLISTO_SEAL_KEY:?generate one with kallisto-ctl gen-key}"
      KALLISTO_CONFIG: /etc/kallisto/kallisto.yaml
      KALLISTO_S3_ACCESS_KEY_ID: "${KALLISTO_S3_ACCESS_KEY_ID:-}"
      KALLISTO_S3_SECRET_ACCESS_KEY: "${KALLISTO_S3_SECRET_ACCESS_KEY:-}"
      KALLISTO_CACHE_DIR: /var/lib/kallisto
    restart: unless-stopped
    cap_drop: [ALL]
    security_opt: ["no-new-privileges:true"]
    read_only: true
    tmpfs: [/tmp]

volumes:
  kallisto-cache:
```

Three details carry the security properties:

- **`127.0.0.1:8200:8200`.** Writing `8200:8200` publishes on every interface.
  Docker's port publishing happens outside Kallisto's own loopback check, so this
  line is the only thing holding the guarantee.
- **`command`.** On a bridge network, a published port forwards to the
  container's own network interface, and Kallisto's default bind is the
  container's loopback, which that forward never reaches. Binding `0.0.0.0`
  inside the container needs `--i-accept-the-risk`. The container's network
  namespace becomes the boundary, and the publish line keeps the host side on
  loopback. Other containers on the same bridge network can then reach port
  8200 too, so keep Kallisto on a network of its own. The alternative is
  `network_mode: host` with the default bind, as the demo uses, on Linux only.
- **`cache-owner`.** Without it the fallback copy fails to write, and the only
  symptom is a line on stderr and a machine that cannot cold-start while the
  bucket is down.

When the reader is another container, you need neither `ports` nor `command`.
Share Kallisto's network namespace and keep the default loopback bind:

```yaml
  app:
    image: my-app
    network_mode: "service:kallisto"
    environment:
      VAULT_ADDR: http://127.0.0.1:8200
```

## Kubernetes

Containers in one pod share a network namespace, so an application container
reaches a sidecar at `127.0.0.1:8200` while Kallisto keeps its default loopback
bind. No `--i-accept-the-risk` and no Service are needed.

:::note[A starting point]
The Kallisto project does not ship Kubernetes manifests. The pod below is
assembled from the constraints on this page. Test it in your cluster before
relying on it.
:::

```yaml
apiVersion: v1
kind: Pod
metadata:
  name: billing
spec:
  securityContext:
    fsGroup: 65532          # makes the cache volume writable by uid 65532
  initContainers:
    # A native sidecar (Kubernetes 1.29+): starts before the app, stops after it.
    - name: kallisto
      image: ghcr.io/alexanderslokov/naughtian-kallisto:latest
      restartPolicy: Always
      env:
        - name: KALLISTO_CONFIG
          value: /etc/kallisto/kallisto.yaml
        - name: KALLISTO_CACHE_DIR
          value: /var/lib/kallisto
        - name: KALLISTO_WORKERS
          value: "1"
        - name: KALLISTO_SEAL_KEY
          valueFrom: { secretKeyRef: { name: kallisto, key: seal-key } }
        - name: KALLISTO_S3_ACCESS_KEY_ID
          valueFrom: { secretKeyRef: { name: kallisto, key: s3-access-key-id } }
        - name: KALLISTO_S3_SECRET_ACCESS_KEY
          valueFrom: { secretKeyRef: { name: kallisto, key: s3-secret-access-key } }
      volumeMounts:
        - { name: config, mountPath: /etc/kallisto, readOnly: true }
        - { name: cache, mountPath: /var/lib/kallisto }
      securityContext:
        readOnlyRootFilesystem: true
        allowPrivilegeEscalation: false
        capabilities: { drop: [ALL] }
      resources:
        limits: { cpu: 500m, memory: 64Mi }
  containers:
    - name: app
      image: my-app
      env:
        - name: VAULT_ADDR
          value: http://127.0.0.1:8200
  volumes:
    - name: config
      configMap: { name: kallisto-config }
    - name: cache
      emptyDir: {}
```

Notes on the choices:

- **`KALLISTO_WORKERS=1`** under a low CPU limit. Each worker is a pinned
  thread, and two workers on half a core only compete. Raise it towards the
  core count on larger limits.
- **Memory.** Measured resident memory is about 8 MiB idle and under 17 MiB at
  saturation with 64 secrets, so a small limit is realistic. Leave headroom for
  larger files: the whole file is held twice over during a refresh.
- **`emptyDir` for the cache** survives container restarts. It does not survive
  the pod moving to another node, where the resolver starts sealed until the
  bucket answers. Use a persistent volume if cold starts during a bucket outage
  matter.
- **A disk source** fits Kubernetes well: mount the sealed file from a `Secret`
  and set `source.type: disk`. No bucket credentials are then needed.

### Health checks

The kubelet sends `httpGet` probes to the pod IP. Kallisto listens on
`127.0.0.1`, so an `httpGet` probe cannot reach it, and the distroless image
has no shell or `curl` for an `exec` probe. Two workable options:

- Leave the sidecar without a probe and let the app's own readiness check call
  `http://127.0.0.1:8200/v1/sys/health`, which answers 200 when a file is loaded
  and 503 when sealed.
- Watch it through metrics instead. See [monitor Kallisto](/kallisto/how-to/monitor-kallisto/).

Binding `0.0.0.0` with `--i-accept-the-risk` so the kubelet can probe it would
also expose the port to every pod that can reach this pod's IP. Avoid that.

## On a plain machine

Run the binary under your service manager with the environment set and the
configuration left at its loopback default:

```bash
KALLISTO_SEAL_KEY=... \
KALLISTO_S3_ACCESS_KEY_ID=... KALLISTO_S3_SECRET_ACCESS_KEY=... \
kallisto-server --config=/etc/kallisto/kallisto.yaml
```

Keep the key out of the command line and out of any file readable by other
users.

## In your application

- **Fetch secrets at runtime.** Kallisto only helps if the application reads
  through it. Baking secrets into a build artefact or a framework cache undoes
  that. Laravel's `config:cache`, for example, writes plaintext secrets into
  `bootstrap/cache`.
- **Fetch per use if you can.** Reading a secret right before using it and
  dropping it right after keeps it out of your process's memory for as long as
  possible. The read is a loopback call to a process holding the value in RAM.
- **Reading once at startup also works.** Set `workers: 1` and spend nothing on
  the read path.
