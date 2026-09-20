# Distributing mTLS credentials

The second use case Kallisto was built for. The first is
[serving KV secrets at a high read rate](./serving-kv-secrets.md); this one uses the same read
path for a different shape of payload.

## The problem

A service mesh needs a leaf certificate and its private key on every workload, rotated often enough
that nobody wants to do it by hand. The usual answers each cost something: baking them into the
image means rebuilding to rotate, mounting them as Kubernetes `Secret` volumes means they sit
decrypted in etcd and refresh on the kubelet's schedule rather than yours, and calling a central CA
at connection setup puts that CA on the request path.

## The shape that fits here

An intermediate CA issues the leaves. An operator seals the current set into one file with
`kallisto-ctl`, puts it on a bucket, and every machine's Kallisto polls it. The workload reads its
certificate and key from `127.0.0.1:8200` using whatever Vault client it already has.

```
  CA / OpenBao ──► operator seals ──► bucket ──► kallisto-server :8200 ──► workload
                   (kallisto-ctl)                (polls, authenticates,
                                                  refuses rollbacks)
```

Rotation is re-sealing. There is no rotation endpoint, because there is no write path — see
[recommended-patterns.md](../../references/internals/recommended-patterns.md).

## Why the read path suits it

Certificates are read at connection setup, which is bursty: a deployment restarts a hundred pods
and they all reach for their key within the same second. A per-worker token bucket with a burst of
one second's worth absorbs exactly that, and the read is a localhost round trip rather than a call
to the CA.

A PEM bundle is also larger than a password, and the read path does not care: the value is stored
as the file's own JSON text and handed out by copying it once, inside the barrier's callback, with
no re-encoding.

## What it does not do

Kallisto has no PKI engine. It does not issue, sign, or renew anything, and it cannot — there is no
write path. It distributes credentials that something else minted. If you want issuance, run
OpenBao's PKI engine and put Kallisto in front of it.

It also has no notion of expiry. A leaf whose validity has run out is served exactly like any other
value until somebody re-seals the file. The file's content version tells you which generation a
machine is on; `kallisto_file_version` across the fleet tells you whether they agree.

## Which credentials belong here

The same test as everything else, from [use-cases.md](./use-cases.md): if this leaks and you revoke
it within five minutes, is the damage contained?

Intermediate CAs for a service mesh and short-lived leaves pass that test. Root CA private keys do
not — they belong in an HSM, and they are read rarely enough that none of Kallisto's advantages
apply.

The threat model, and what the in-RAM barrier is and is not worth for a private key, is in
[security.md](../security.md).
