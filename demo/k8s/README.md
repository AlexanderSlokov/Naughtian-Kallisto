# Kallisto on Kubernetes: a fleet demo

Twenty pods. Each one is an app plus its own Kallisto sidecar. The app is the stock Vault CLI pointed at
`127.0.0.1:8200`; it does not know it is not talking to Vault. Every Kallisto polls the same sealed file
in a bucket (Garage) every 5 seconds.

The compose demo in `..` shows one Kallisto. This one shows what only a fleet can: what happens to all
of them at once.

## Before you start

- A cluster. The scripts default to the context `k3d-kallisto`:

  ```bash
  k3d cluster create kallisto --servers 1 --agents 0 \
    --k3s-arg "--disable=traefik@server:0" --kubeconfig-switch-context=false
  ```

  Any other cluster works with `KALLISTO_DEMO_CONTEXT=<context>`. The scripts never change your current
  context.
- Kubernetes 1.29 or later, for native sidecars.
- `kubectl`, and `k9s` if you want to watch.

All credentials here are deliberately fake: all-zero hex of the right length. Do not use them anywhere.

## Run it

```bash
./up.sh     # Garage, the bucket, version 1 of the file, 20 pods. About a minute.
```

Watch any pod's `app` container log (`l` in k9s). Every two seconds it prints:

```
14:22:04 file=v1 password=v1-password-not-a-real-credential
```

## Act 1: rotate a secret for the whole fleet

```bash
./act-1-rotate.sh
```

It seals version 2 of the file with a new password and uploads it. Then it waits until every pod serves it:

```
[demo] Uploaded. Waiting for every pod to serve it:
[demo] t+2s  20 pods: v1=13 v2=7
[demo] t+5s  20 pods: v2=20
[demo] Password now, from fleet-64dd5db589-2tjhf: v2-password-rotated-not-a-real-credential
[demo] Container restarts during the act: 0
```

In the app log the change is one line to the next:

```
14:22:04 file=v1 password=v1-password-not-a-real-credential
14:22:06 file=v2 password=v2-password-rotated-not-a-real-credential
```

What that shows: the new secret reached every pod within one poll interval. No pod restarted, nothing
pushed to the pods, and there is no server in the middle for them to depend on.

To run the act again, start over with `./down.sh && ./up.sh`. Uploading version 2 a second time changes
nothing, correctly: a version Kallisto already holds is not even opened.

Look at the `t+2s` line again: for a few seconds part of the fleet holds the old password and part the
new one. That is what rotating through a fleet means, and it has a consequence outside Kallisto: the
database has to accept both passwords until every pod has moved. See [What this shows, and what it does
not](#what-this-shows-and-what-it-does-not).

## Act 2: the bucket goes down

```bash
./act-2-bucket-down.sh
```

Runs after act 1, on a fleet serving version 2. Four steps:

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

1. Garage is gone. Every Kallisto logs `source is unreachable: connection refused` on each poll, and every
   app keeps reading the secret. The file was already in RAM; the bucket is only asked whether it changed.
2. One Kallisto is killed (`crictl stop` on the node) while the bucket is still down. The kubelet restarts
   it, it reads the encrypted copy it wrote to its cache directory, and serves version 2 again. The app
   loses one read, the two seconds the container takes to come back.
3. A new pod starts while the bucket is down. It has no cache and no bucket, so it has nothing to serve:
   Kallisto answers 503 and the Vault CLI prints `* Vault is sealed`. That is the right answer. The wrong
   part is the line under it: Kubernetes reports all 21 pods `2/2 Running`, the sealed one included,
   because there is no probe to tell it otherwise. Behind a Service, that pod would take traffic.
4. Garage comes back empty, and Kallisto logs `no sealed file at the configured location` until the file
   is uploaded again. Then the new pod loads it within one poll.

Step 2 needs a node that `docker exec` reaches, as with k3d or kind: the image is distroless, so there is
no shell inside it to kill the process from.

The cache lives in an `emptyDir`, which survives a container restart and dies with the pod. So it saves a
crashed Kallisto, as in step 2, but not a pod that is rescheduled, as in step 3. A cache that outlives the
pod would need a volume that does.

## What this shows, and what it does not

Shown:

- The stock Vault CLI (`hashicorp/vault:1.20`, unmodified) cannot tell Kallisto from Vault. It discovers the
  mount through `sys/internal/ui/mounts` and reads KV-v2 the way it would from Vault. That is
  compatibility as a client sees it; it says nothing about Vault itself.
- One upload reaches the whole fleet within one poll interval, with no restart, no push and no server in
  the middle.
- A fleet that already holds the file keeps serving with the bucket gone, and a crashed Kallisto comes back
  from its own encrypted copy.

Not shown:

- The apps are a shell loop that asks for the secret every two seconds. An app that reads its secrets once
  at startup and keeps them never sees version 2. Kallisto makes the new value available; the app still
  has to read it again.
- Kallisto distributes a new secret; it does not rotate it. Changing the password in the database is
  someone else's job, and because the fleet moves over a few seconds, both passwords have to work during
  that window.
- No authorization. The file carries no token table, so every read is permitted. Token-scoped reads have
  not been run on Kubernetes.
- Only the Vault CLI. The Go, PHP and Python SDKs are tested in `tests/duck`, not on this cluster.
- Nothing about performance. k3d runs its node as a Docker container, here inside one laptop, so no
  number from it means anything for production.

## Clean up

```bash
./down.sh   # deletes the namespace; the cluster stays
k3d cluster delete kallisto
```

## Files

- `bucket.yaml`: Garage, single node, and the fake credentials.
- `fleet.yaml`: the 20 pods and the Kallisto configuration they share.
- `seal-job.yaml`: seals `secrets.v<N>.json` with `kallisto-ctl` and uploads it with `rclone`.
- `lib.sh`: what the scripts share, including how the fleet's versions are counted: `v<N>`, `sealed` for a
  Kallisto that answered 503, `down` for one that did not answer.

## Known gap

The pods have no probes. Kallisto listens on loopback only, the kubelet probes the pod IP, and the image
is distroless, so there is no shell for an exec probe either. Two consequences, both visible above: an
app's first lines may show a connection error while its Kallisto loads the file, and a pod whose Kallisto
holds nothing (act 2, step 3) still counts as Running and ready.
