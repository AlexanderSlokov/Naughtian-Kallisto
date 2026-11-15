---
title: Seal and publish a secrets file
description: Generate a seal key, write the plaintext, seal it with kallisto-ctl, upload it, and roll out a new version.
sidebar:
  order: 1
---

Everything that writes a sealed file happens offline, in `kallisto-ctl`. The
server only ever reads. This guide goes from nothing to a file in a bucket that
a resolver can serve, and then through a rotation.

`kallisto-ctl` ships in the container image at `/usr/local/bin/kallisto-ctl`, or
you can [build it from source](/kallisto/how-to/build-from-source/).

## 1. Generate a seal key

```bash
kallisto-ctl gen-key
```

It prints 32 random bytes, hex-encoded, on stdout and stores them nowhere. Put
the value straight into your secret store: the orchestrator's secrets, a CI
secret, a password manager. Every machine that serves the file needs it as
`KALLISTO_SEAL_KEY`.

For this session:

```bash
export KALLISTO_SEAL_KEY=$(kallisto-ctl gen-key)
```

The key is only ever read from the environment. `--seal-key` exists solely to be
refused, because command-line arguments are readable by every process on the
host through `ps`.

## 2. Write the plaintext

```json
{
  "version": 1,
  "secrets": {
    "app/db": { "username": "app", "password": "change-me" },
    "app/stripe": { "webhook_secret": "change-me" }
  },
  "policies": {},
  "tokens": {}
}
```

- `version` is the content version. It must go up every time you publish.
- Each key in `secrets` is a path without the mount prefix. `app/db` is served
  at `/v1/secret/data/app/db`. Each value is the KV-v2 `data` object.
- Empty `policies` and `tokens` mean every read is permitted. That fits one app
  with its own file. To give several apps different access, see [add tokens and
  policies](/kallisto/how-to/add-tokens-and-policies/).

The full schema is in the [sealed file
reference](/kallisto/references/sealed-file/). Keep the plaintext out of git, or
keep it encrypted there.

## 3. Seal it

```bash
kallisto-ctl seal --in plain.json --out secrets.kal
```

```
sealed version 1 into secrets.kal: 2 secret(s), 0 polic(ies), 0 token(s)
```

Check the result without printing anything sensitive:

```bash
kallisto-ctl verify --in secrets.kal
```

```
secrets.kal: authentic
  version   1
  secrets   2
  policies  0
  tokens    0
```

A last line reports authorization: `none` when the file has no token table and
every read is permitted, `enforced` when it has one. `verify` checks the
authentication tag and reports counts. It never prints a secret or a path.

## 4. Upload it

Any S3-compatible tool works. For example:

```bash
aws s3 cp secrets.kal s3://kallisto/prod/payment.kal
# or
mc cp secrets.kal myminio/kallisto/prod/payment.kal
```

Give the credential each resolver uses **read-only** access to that one object.
Write access to the bucket is the attacker position the design defends against,
so no resolver should hold it.

Point the resolver at the object in its configuration. See [configure the
source](/kallisto/how-to/configure-the-source/).

## 5. Roll out a change

Rotating a secret, revoking a token and changing a policy are all the same
motion: edit the plaintext, raise the version, seal, upload.

```bash
# after editing plain.json and raising "version" to 2
kallisto-ctl seal --in plain.json --out secrets.kal
aws s3 cp secrets.kal s3://kallisto/prod/payment.kal
```

Every resolver picks it up on its next poll, 30 seconds by default.
`kallisto_file_version` on `/v1/sys/health` and `/v1/sys/metrics` shows which
version each machine is serving.

If the file already on disk holds the same or a higher version, `seal` stops,
because a resolver would refuse the result. You can also raise the version in
place on an existing sealed file:

```bash
kallisto-ctl bump-version --in secrets.kal          # +1
kallisto-ctl bump-version --in secrets.kal --to 10  # a specific version
```

:::caution[Why the version matters]
A resolver treats a file with the version it already holds as the same file and
never opens it. A file with a lower version is refused as a rollback. Edit
values without raising the version and nothing changes on any machine.
:::

`seal --force` overwrites an existing output regardless of its version. Use it
only when rebuilding from scratch, with resolvers that have never seen the
older, higher version.

## Rotating the seal key

The seal key and the token key are independent, so rotating the seal key leaves
every token working.

1. Generate a new key.
2. Seal the plaintext under it, with a higher version.
3. Upload the new file.
4. Update `KALLISTO_SEAL_KEY` on each machine and restart its resolver.

Keep that order. Between steps 3 and 4, a resolver still holding the old key
cannot authenticate the new file, so it keeps serving the table it already has
and counts each failed poll in `kallisto_refresh_failures_total`. After the
restart, its fallback copy is still under the old key: it logs
`sealed file failed authentication` for the cache, then loads the new file from
the bucket.

Reversing steps 3 and 4 means a restarted resolver can authenticate neither
the bucket's file nor its fallback copy, and it stays sealed until the upload.

## Reading a file back

```bash
kallisto-ctl open --in secrets.kal --yes-print-secrets-to-stdout
```

Prints the whole plaintext. The flag is spelled out so it cannot be typed by
accident or left unnoticed in a script.
