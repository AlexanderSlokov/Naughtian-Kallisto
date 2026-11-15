---
title: Add tokens and policies
description: Let several applications share one sealed file while each reads only its own paths.
sidebar:
  order: 3
---

A file with no token table serves every read to whoever can reach the loopback
port. That suits one app with its own file, where the bucket credential is the
boundary. When several apps on one machine share a file and must not read each
other's secrets, add a token table.

:::tip[Consider splitting the file first]
One file per trust boundary is usually cheaper than a token table: one more
object in the bucket, nothing to mint, and a stolen bucket credential exposes
only that app's secrets.
:::

## 1. Add a token key to the plaintext

The token key is separate from the seal key. Generate one:

```bash
kallisto-ctl gen-key
```

Add it to the plaintext as `token_key` (snake case, unlike the camelCase
configuration file):

```json
{
  "version": 3,
  "token_key": "<hex from gen-key>",
  "secrets": {
    "billing/db": { "password": "change-me" },
    "search/api": { "key": "change-me" }
  },
  "policies": {},
  "tokens": {}
}
```

The token key lives inside the sealed file. Rotating the seal key re-seals it
with everything else, so every token keeps working through a seal key rotation.

## 2. Write the policies

Policies use Vault's own path syntax, including the KV-v2 quirk: reading is
granted on `<mount>/data/...` and listing on `<mount>/metadata/...`.

```json
"policies": {
  "billing": [
    { "path": "secret/data/billing/*",     "capabilities": ["read"] },
    { "path": "secret/metadata/billing/*", "capabilities": ["list"] }
  ],
  "search": [
    { "path": "secret/data/search/*", "capabilities": ["read"] }
  ]
}
```

- Three capabilities mean something here: `read`, `list` and `deny`. `create`,
  `update` or `sudo` still parse and grant nothing, because there is no write
  path.
- `*` at the end matches anything, across `/`, as in Vault. `+` matches exactly
  one path segment.
- `deny` wins over every other rule, however broad it is. Vault resolves
  conflicts by path specificity instead, so on a contradictory policy Kallisto
  refuses a request Vault would allow. On any policy without a contradiction,
  the two agree.

A policy written for a real Vault works here unchanged, and one written here
keeps working after a move to OpenBao.

## 3. Seal once, then mint tokens

`mint-token` reads the token key from a sealed file, so seal first:

```bash
kallisto-ctl seal --in plain.json --out secrets.kal
kallisto-ctl mint-token --in secrets.kal --policy billing
```

On stderr it prints the line to add to the plaintext:

```
Add to the plaintext file's "tokens" map, then re-seal:

    "5f1c...e9": ["billing"]
```

On stdout it prints the token itself, `s.` followed by 64 hex characters. It is
shown once. The file stores only a keyed hash, so a lost token cannot be
recovered and has to be replaced by a new one.

Because only the token goes to stdout, you can pipe it straight into a secret
store:

```bash
kallisto-ctl mint-token --in secrets.kal --policy billing | store-token-somewhere
```

`--policy` can be repeated to attach several policies. To mint before any file
carries a key, set `KALLISTO_TOKEN_KEY` and leave out `--in`.

## 4. Paste the rows and re-seal

Add each printed row to `tokens`, raise `version`, and seal and upload again:

```json
"tokens": {
  "5f1c...e9": ["billing"],
  "a03b...71": ["search"]
}
```

`seal` refuses a file with tokens and no `token_key`, since no token could ever
authenticate against it. It warns when a token names a policy the file does not
define. Such a token grants nothing.

## 5. Give each app its token

Apps present the token the way they would to Vault: the `X-Vault-Token` header,
or `Authorization: Bearer <token>`. Vault SDKs do this from `VAULT_TOKEN`:

```bash
VAULT_ADDR=http://127.0.0.1:8200
VAULT_TOKEN=s.<the billing token>
```

Once the file has a token table, authorization is default deny, including for a
request that carries no token. The permission check runs before the lookup, so
a 403 says nothing about whether a path exists.

Confirm the switch on the health endpoint:

```bash
curl -s http://127.0.0.1:8200/v1/sys/health | python3 -m json.tool | grep authorization
#   "kallisto_authorization": "enforced",
```

## Revoking a token

Delete its row from `tokens`, raise the version, seal and upload. Every
resolver stops accepting it on its next poll. There is no revocation endpoint,
and no token expiry: a token lasts until its row is removed.
