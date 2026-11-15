---
title: The sealed file
description: The plaintext JSON schema that kallisto-ctl seals, and the binary layout of the result.
sidebar:
  order: 3
---

A Kallisto deployment serves exactly one sealed file. It carries the secrets,
the policies, the token table and the key those tokens were hashed with, all
encrypted together.

## Plaintext schema

The JSON that `kallisto-ctl seal` reads:

```json
{
  "version": 4,
  "token_key": "9c1e...64 hex characters...",
  "secrets": {
    "app/db": { "username": "app", "password": "..." },
    "app/api": { "key": "..." }
  },
  "policies": {
    "app": [
      { "path": "secret/data/app/*",     "capabilities": ["read"] },
      { "path": "secret/metadata/app/*", "capabilities": ["list"] }
    ]
  },
  "tokens": {
    "5f1c...keyed hash...": ["app"]
  }
}
```

| Field | Type | Required | Meaning |
|---|---|---|---|
| `version` | integer | yes | Content version. Must rise with every publish. Never reused. |
| `secrets` | object | no | Path to KV-v2 `data` object. Paths omit the mount and `data/` prefix. |
| `policies` | object | no | Policy name to a list of rules. |
| `tokens` | object | no | Keyed token hash to a list of policy names. |
| `token_key` | hex string | with tokens | Key for the token hashes, 32 bytes hex-encoded. |

Field names are snake case, unlike the camelCase server configuration. Unknown
fields are refused.

### Rules

Each rule has a `path` and `capabilities`.

- `path` uses Vault policy syntax against the full API path after `/v1/`,
  including the mount and the `data/` or `metadata/` segment. A trailing `*`
  matches anything, across `/`. `+` matches exactly one segment.
- `capabilities` that grant something here: `read` (on `data/`), `list` (on
  `metadata/`) and `deny`. Other Vault capabilities parse and grant nothing.
- `deny` takes precedence over every other matching rule.

### Authorization modes

| `token_key` | `tokens` | Result |
|---|---|---|
| absent | empty | No authorization. Every read is permitted. Reported as `"kallisto_authorization": "none"`. |
| present | any | Default deny. A request needs a token whose policies allow the path. |
| absent | non-empty | Refused by `seal`, and by the resolver if it ever meets one. |

## Binary layout

```
┌──────────┬────────────────┬─────────────────┬──────────┬────────────────────┐
│ KALLISTO │ format version │ content version │  nonce   │ AES-256-GCM        │
│  magic   │                │  u64, LE        │ 12 bytes │ ciphertext + tag   │
└──────────┴────────────────┴─────────────────┴──────────┴────────────────────┘
 └──────────── plaintext header, passed as additional authenticated data ───┘
```

The header is readable without the key, so a resolver can compare the content
version before decrypting anything. It is also authenticated, so changing any
header byte, the version included, fails the tag check.

## Rules the resolver applies

- A file that fails authentication is refused.
- A file with a version older than the one being served is refused as a
  rollback.
- A file with the same version as the one being served is treated as unchanged
  and never opened.
- Each refusal leaves the current table in place and increments
  `kallisto_refresh_failures_total`.

## Size

No cap is configured. The design assumes a few dozen secrets, read almost
exclusively, held in a plain hash map. During a refresh the file is held in
memory twice: the decrypted plaintext briefly, and the per-secret sealed copies
that replace it.
