---
title: kallisto-ctl
description: The offline tool that creates, checks and inspects sealed files, mints tokens and validates configuration.
sidebar:
  order: 2
---

`kallisto-ctl` is the offline half of Kallisto. Everything that writes a sealed
file happens here and never over the network, because the server has no
endpoint that writes.

```bash
kallisto-ctl <command> [flags]
```

Flags accept `--flag value` and `--flag=value`. An unrecognised flag stops the
command. `--seal-key` and `--token-key` are refused by name: keys come from the
environment.

## Environment

| Variable | Used by |
|---|---|
| `KALLISTO_SEAL_KEY` | `seal`, `verify`, `bump-version`, `open`, and `mint-token --in` |
| `KALLISTO_TOKEN_KEY` | `mint-token` without `--in` |

## Commands

### `gen-key`

```bash
kallisto-ctl gen-key
```

Prints 32 bytes from the system RNG, hex-encoded, on stdout. Use the value as
`KALLISTO_SEAL_KEY` or as a file's `token_key`. Nothing is stored.

### `seal`

```bash
kallisto-ctl seal --in plain.json --out secrets.kal [--version N] [--force]
```

Encrypts a plaintext file with AES-256-GCM under `KALLISTO_SEAL_KEY`, writing
the output atomically.

- `--version N` overrides the `version` in the plaintext.
- If `--out` already exists with the same or a higher version, `seal` stops,
  because a resolver would refuse the result as a rollback. `--force` writes it
  anyway.
- A file with tokens and no `token_key` is refused. A token naming an
  undefined policy produces a warning.

### `verify`

```bash
kallisto-ctl verify --in secrets.kal
```

Checks the authentication tag and prints the content version, counts of
secrets, policies and tokens, and whether authorization is enforced. It never
prints a secret or a secret's path. An authentication failure means the key is
wrong or the file was altered. The two cannot be told apart, by design.

### `bump-version`

```bash
kallisto-ctl bump-version --in secrets.kal [--to N]
```

Raises the content version in place, by one or to `N`. `N` must be higher than
the current version. The version is part of the authenticated header, so the
file is opened and re-sealed, which needs the key.

### `mint-token`

```bash
kallisto-ctl mint-token --in secrets.kal --policy <name> [--policy <name> ...]
```

Generates a token (`s.` followed by 64 hex characters) and its keyed hash.

- stdout: the token, shown once.
- stderr: the row to paste into the plaintext's `tokens` map.

The token key comes from the sealed file given with `--in`. Without `--in`, it
comes from `KALLISTO_TOKEN_KEY`, for minting before any file carries a key. At
least one `--policy` is required.

### `validate`

```bash
kallisto-ctl validate --config kallisto.yaml
```

Parses and resolves a server configuration and prints what the server would
do: listen address, workers, mount, source, refresh interval, cache path,
limits and log settings. It uses the file alone and ignores the current
environment and flags, so the result does not depend on the shell it runs in.

### `open`

```bash
kallisto-ctl open --in secrets.kal --yes-print-secrets-to-stdout
```

Decrypts and prints the whole plaintext as JSON. Without the flag it refuses.
The output lands in terminal scrollback and session logs, so treat the terminal
accordingly.

## Exit behaviour

Usage errors (a missing flag, a version that would go backwards) and failures
(unreadable file, authentication failure) exit non-zero with a message on
stderr. Messages about a sealed file never quote its contents.
