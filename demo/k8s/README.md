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

## Clean up

```bash
./down.sh   # deletes the namespace; the cluster stays
k3d cluster delete kallisto
```

## Files

- `bucket.yaml`: Garage, single node, and the fake credentials.
- `fleet.yaml`: the 20 pods and the Kallisto configuration they share.
- `seal-job.yaml`: seals `secrets.v<N>.json` with `kallisto-ctl` and uploads it with `rclone`.
- `lib.sh`: what the scripts share, including how the fleet's versions are counted.

## Known gap

The pods have no probes. Kallisto listens on loopback only, the kubelet probes the pod IP, and the image
is distroless, so there is no shell for an exec probe either. Until that is solved, an app's first lines
may show a connection error while its Kallisto loads the file.
