# Kallisto on Kubernetes: a fleet demonstration

This directory holds a reproducible demonstration of Kallisto deployed as a sidecar across a fleet of
twenty Kubernetes pods. Three experiments examine how the fleet behaves when a secret is rotated, when
its storage becomes unavailable, and when someone with write access to that storage, but without the
seal key, tries to substitute the file. The results reported below were recorded on 18 October 2026
with Kallisto 2.0.2. The last two sections state what the experiments do not show and one deployment
gap they exposed.

## 1. System under test

Kallisto is a local, read-only secrets resolver that answers the HashiCorp Vault KV-v2 HTTP API
([ADR-0015](../../docs/references/ADRs/ADR-0015.md)). It serves secrets from one sealed file, kept in an
S3-compatible bucket, and never writes to it. Three properties of its design are under test here.

- Each Kallisto polls the bucket on a fixed interval and atomically replaces the table it serves when a
  newer valid file appears. Nothing is pushed to it, and no server coordinates the fleet.
- Once a file is loaded it is served from memory. The bucket is consulted only to learn whether the
  file has changed. Each valid file is also written, still encrypted, to a local cache directory, from
  which a restarted process recovers before it contacts the bucket.
- The sealed file is authenticated encryption, AES-256-GCM. Its 29-byte header holds the magic
  `KALLISTO`, a format byte, the content version as a little-endian 64-bit integer at offset 9, and the
  12-byte nonce. The header is passed to the cipher as associated data, so it is authenticated together
  with the body. Before decrypting, Kallisto refuses any file whose header version is lower than the
  version it already serves (`components/kallisto_crypto/src/sealed_file.rs`, `open`).

## 2. Experimental setup

### 2.1 Environment

| Component | Version |
|---|---|
| Host | AMD Ryzen 5 3550H (4 cores, 8 threads), 13 GiB RAM, Ubuntu 24.04.5, Linux 7.0.0-38 |
| Container runtime | Docker 29.8.2 |
| Kubernetes | k3s v1.35.5+k3s1 in k3d v5.9.0, one server node, no agents |
| Kallisto | `ghcr.io/alexanderslokov/naughtian-kallisto:2.0.2` |
| Object storage | Garage v2.4.1 (`dxflrs/garage`), single node |
| Client | Vault CLI v1.20.4 (`hashicorp/vault:1.20`), unmodified |
| Upload tool | rclone 1.71 |

Garage is used because MinIO no longer publishes container images; at the time of writing neither
`quay.io/minio/minio` nor `minio/minio` can be pulled.

### 2.2 Topology

```
                       Garage (one bucket, one object: demo.kal)
                                        ^
                                        | GET, every 5 s, from each sidecar
            +---------------------------+---------------------------+
            |                           |                           |
   pod 1                       pod 2                ...    pod 20
   app: vault kv get  --127.0.0.1:8200-->  kallisto (native sidecar)
```

Each of the twenty pods (`fleet.yaml`) contains two containers that share the pod's network namespace.

- `kallisto` runs as a native sidecar, an init container with `restartPolicy: Always`, so it starts
  before the application and lives as long as the pod. It listens on `127.0.0.1:8200` with one worker,
  polls every 5 seconds, and caches the sealed file in an `emptyDir`. It runs as a non-root user with a
  read-only root filesystem and no Linux capabilities.
- `app` stands in for an application. Every 2 seconds it reads `secret/app/database` with
  `vault kv get` and prints one line with the file version and the password it received.

New files are sealed with `kallisto-ctl` and uploaded by a Kubernetes Job (`seal-job.yaml`). No pod in
the demonstration mounts a service account token.

### 2.3 Measurement

The scripts poll every pod's `GET /v1/sys/health` through `kubectl exec` and classify the answer as
`v<N>` (the file version served), `sealed` (Kallisto answered 503 because it holds no file) or `down`
(no answer). One sweep of the fleet takes about three seconds, so the times reported below have that
resolution. They are upper bounds on propagation, not exact measurements.

### 2.4 Credentials

Every credential in this directory is a deliberate placeholder: all-zero hexadecimal of the length each
field requires. None of them may be used outside this demonstration.

## 3. Results

### 3.1 Experiment 1: rotating a secret across the fleet (`act-1-rotate.sh`)

Procedure. With every pod serving version 1, version 2 of the file, which carries a different password,
is sealed and uploaded once.

Observation.

```
[demo] Uploaded. Waiting for every pod to serve it:
[demo] t+2s  20 pods: v1=13 v2=7
[demo] t+5s  20 pods: v2=20
[demo] Password now, from fleet-64dd5db589-2tjhf: v2-password-rotated-not-a-real-credential
[demo] Container restarts during the act: 0
```

In the application log of a single pod the change appears between two consecutive reads:

```
14:22:04 file=v1 password=v1-password-not-a-real-credential
14:22:06 file=v2 password=v2-password-rotated-not-a-real-credential
```

Interpretation. All twenty pods served the new password within one poll interval of the upload, with
no container restarted and no component pushing to the pods. Because the sidecars poll independently,
their transitions are spread over that interval. For about five seconds the fleet therefore held both
the old and the new password (`v1=13 v2=7` at t+2s), a point taken up in section 4.

### 3.2 Experiment 2: loss of the bucket (`act-2-bucket-down.sh`)

Procedure. Starting from a fleet serving version 2: (1) Garage is scaled to zero replicas; (2) while it
is down, one Kallisto container is stopped with `crictl stop` on the node, as a crash would stop it,
and the kubelet restarts it; (3) while it is still down, a twenty-first pod is started; (4) Garage is
restored and the file uploaded again.

Observation.

```
[demo] 1/4 Taking the bucket down
[demo]   Bucket is gone. Every poll now fails, and the fleet keeps answering:
[demo]   20 pods: v2=20
[demo] 2/4 Crashing the Kallisto in fleet-64dd5db589-2tjhf, bucket still down
[demo]   It came back: kallisto: loaded version 2 from cache
[demo]   20 pods: v2=20
[demo] 3/4 A brand-new pod, bucket still down
[demo]   21 pods: sealed=1 v2=20
[demo]   What Kubernetes thinks of them:
             21 2/2 Running
[demo] 4/4 Bucket back
[demo] t+2s  21 pods: v2=21
[demo]   Back to 20 pods.
```

Interpretation, by step.

1. With the bucket unreachable, every Kallisto logged `source is unreachable: connection refused` on each
   poll, and every application read succeeded. The bucket is not on the read path.
2. The restarted Kallisto recovered version 2 from its encrypted cache without contacting the bucket.
   Its application lost one read, during the roughly two seconds the container took to restart.
3. The new pod had neither a cache nor a bucket, and so had nothing to serve. Kallisto answered 503, which
   the Vault CLI reports as `* Vault is sealed`; this is the intended behaviour. Kubernetes nevertheless
   reported all twenty-one pods as `2/2 Running`, the sealed one included, because the pods have no
   probes (section 6).
4. When Garage came back empty, Kallisto logged `no sealed file at the configured location` until the
   file was uploaded again, after which the new pod loaded it within one poll.

The cache is an `emptyDir`, which survives a container restart and is deleted with the pod. It therefore
protected the crashed container in step 2 but could not help the new pod in step 3. A cache that
outlives the pod would require a persistent volume.

### 3.3 Experiment 3: substitution by a party without the key (`act-3-rollback.sh`)

Procedure. The adversary can write to the bucket but does not hold the seal key. Starting from a fleet
serving version 2, three files are uploaded in turn, each replacing the object the fleet polls:

1. Replay: the genuine version 1 file, validly sealed, uploaded again.
2. Forgery: a version 99 file sealed with a different key (thirty-two `0xff` bytes).
3. Tampering: the genuine version 1 file with the version in its header rewritten to 3, so that it claims
   to be newer than the version served.

Observation.

```
[demo] 1/3 Replay: the genuine version 1 file, uploaded again
[demo]   20 pods: v2=20
[demo]   Password served: v2-password-rotated-not-a-real-credential
[demo]   Kallisto logged: source: refused the file offered by source: sealed file refused: refusing an older sealed file: holding version 2, offered 1
[demo] 2/3 Forgery: version 99, sealed with a key that is not the fleet's
[demo]   20 pods: v2=20
[demo]   Password served: v2-password-rotated-not-a-real-credential
[demo]   Kallisto logged: source: refused the file offered by source: sealed file refused: sealed file failed authentication
[demo] 3/3 Tampering: the genuine version 1 file, header rewritten to claim version 3
[demo]   20 pods: v2=20
[demo]   Password served: v2-password-rotated-not-a-real-credential
[demo]   Kallisto logged: source: refused the file offered by source: sealed file refused: sealed file failed authentication
```

Interpretation. The fleet served version 2 throughout, and each attempt was refused by the check it was
designed to meet.

- The replayed file is authentic but older. It is refused by the version comparison, which runs before
  any decryption.
- The forged file passes the version comparison, since 99 is higher than 2, and fails authentication,
  since it was not produced with the fleet's key.
- The tampered file is the most informative case. Its log line reports an authentication failure, not
  `offered 1`, which confirms that the rewritten header was read: version 3 passed the version
  comparison. It then failed authentication because the header is associated data of the cipher, and
  changing a single byte of it invalidates the tag. The version number cannot be altered independently
  of the key.

Each refusal is logged again on every poll for as long as the refused object remains in the bucket, so
a substitution stays visible in the logs rather than appearing once.

### 3.4 Resource use

Measured with `kubectl top` (container working-set memory, as reported by metrics-server) on the twenty
idle sidecars at the end of the experiments: mean 3.1 MiB, range 3 to 4 MiB, about 1 millicore of CPU
each. This configuration uses one worker and no access log. The 8.3 MiB quoted in the repository's main
README is a resident set size measured on the host with two workers, and the two figures are not
directly comparable.

## 4. Discussion

The experiments support three claims.

1. Client compatibility. An unmodified Vault CLI discovered the mount through
   `sys/internal/ui/mounts` and read KV-v2 secrets from Kallisto as it would from Vault. This is
   compatibility as seen by a client; it implies nothing about Vault itself.
2. Distribution without a central server. One upload reached the entire fleet within one poll interval,
   without restarts, pushes or a coordinating service.
3. Availability and integrity under bucket failure or compromise. A fleet that has loaded a file keeps
   serving it when the bucket is lost, and a party who can write to the bucket but lacks the key cannot
   make the fleet serve an older, forged or altered file.

Two consequences follow for anyone deploying this pattern. First, rotating a credential through a polling
fleet produces a window, here up to one poll interval, in which both the old and the new value are in
use. The system that consumes the credential, a database for instance, must accept both for that window.
Second, a party with write access to the bucket cannot change what the fleet serves, but it can stop the
fleet from receiving new versions, by deleting the object or replacing it with something that will be
refused. Integrity is protected; the timeliness of updates depends on the bucket's own access control.

## 5. Limitations

- The applications are a shell loop that requests the secret every two seconds. An application that
  reads its secrets once at startup and keeps them would not observe version 2 until it reads again.
  Kallisto makes a new value available; it does not make applications use it.
- Kallisto distributes secrets; it does not rotate them. Changing the credential at its origin is outside
  its scope.
- No authorization was exercised. The file carries no token table, so every read is permitted.
  Token-scoped reads have not been tested on Kubernetes.
- Only the Vault CLI was used as a client. The Go, PHP and Python SDKs are covered by `tests/duck`, not by
  this setup.
- The cluster is a single k3d node running as a Docker container on a laptop. No figure from it,
  including the propagation times above, should be read as a performance result.
- Step 2 of experiment 2 requires a node reachable with `docker exec`, as in k3d or kind, because the
  Kallisto image is distroless and contains no shell from which to stop the process.

## 6. Deployment gap: no health probes

The pods define no liveness, readiness or startup probes. Kallisto binds only the loopback interface, by
design, while the kubelet sends HTTP probes to the pod IP; and the image is distroless, so there is no
shell or HTTP client for an exec probe. Two effects were observed. An application's first reads can fail
while its Kallisto is still loading the file, and a pod whose Kallisto holds no file (experiment 2, step
3) is still reported as running and ready, so a Service would route traffic to it. Closing this gap
requires a probe mechanism that Kallisto does not yet provide.

## 7. Reproducing the results

Requirements: Kubernetes 1.29 or later (for native sidecars), `kubectl`, and for experiment 2 a node
reachable with `docker exec`. The scripts use the context `k3d-kallisto` unless `KALLISTO_DEMO_CONTEXT`
names another, and never change the current context.

```bash
k3d cluster create kallisto --servers 1 --agents 0 \
  --k3s-arg "--disable=traefik@server:0" --kubeconfig-switch-context=false

./up.sh                   # Garage, the bucket, version 1, twenty pods; about one minute
./act-1-rotate.sh         # experiment 1
./act-2-bucket-down.sh    # experiment 2; expects the fleet on version 2
./act-3-rollback.sh       # experiment 3; expects the fleet on version 2

./down.sh                 # deletes the namespace; the cluster remains
k3d cluster delete kallisto
```

The experiments are meant to run in this order. To repeat experiment 1, start again from `./up.sh`
after `./down.sh`: a file whose version Kallisto already holds is not opened, so uploading version 2 a
second time changes nothing.

Any pod's `app` container log (`l` in k9s) shows the reads as they happen; its `kallisto` container
log shows each poll that fails or is refused.

## 8. Files

- `bucket.yaml`: the namespace, the placeholder credentials and a single-node Garage.
- `fleet.yaml`: the twenty pods and the Kallisto configuration they share.
- `seal-job.yaml`: a Job template that seals `secrets.v<N>.json` with `kallisto-ctl` and uploads it with
  rclone. It can seal with another key, and rewrite the header version, for experiment 3.
- `secrets.v1.json`, `secrets.v2.json`, `secrets.v99.json`: the plaintext of each version; v99 is the
  forgery.
- `lib.sh`: functions shared by the scripts, including the fleet sweep described in section 2.3.
- `up.sh`, `act-1-rotate.sh`, `act-2-bucket-down.sh`, `act-3-rollback.sh`, `down.sh`: the procedure.

## References

- ADR-0015, the repositioning of Kallisto as a local read-only resolver: `docs/references/ADRs/ADR-0015.md`.
- ADR-0016, the read path and thread-per-core: `docs/references/ADRs/ADR-0016.md`.
- M. Dworkin, *Recommendation for Block Cipher Modes of Operation: Galois/Counter Mode (GCM) and GMAC*,
  NIST Special Publication 800-38D, 2007.
- D. McGrew, *An Interface and Algorithms for Authenticated Encryption*, RFC 5116, IETF, 2008.
- Kubernetes documentation, *Sidecar Containers*: https://kubernetes.io/docs/concepts/workloads/pods/sidecar-containers/
