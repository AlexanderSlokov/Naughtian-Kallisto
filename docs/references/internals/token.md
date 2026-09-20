# Tokens and policies

ADR-0015 D8 turns Vault's one genuinely stateful subsystem into data. There is no token store, no
lease, no expiry, and no revocation endpoint. The sealed file carries a table of
`hash(token) → policy names`, an operator mints a random token and hands it to an app, and revoking
it means deleting that line and re-sealing — the same motion as rotating a secret.

Implementation: `components/kallisto_policy`.

## The hash is keyed, and the key is inside the file

Two consequences, both deliberate.

The plaintext of the file is authored by hand and passes through somebody's editor, `git`, and a CI
job before it is ever sealed. Because the hashes in it are keyed, they cannot be attacked offline
without the key, even if an operator picks a short token.

Rotating the *seal* key does not invalidate a single token, because the token key is re-sealed
along with everything else. Deriving the token key from the seal key would have made every rotation
a fleet-wide outage: the operator holds the hashes, not the tokens, so there is nothing to
recompute them from.

The key is used under a domain-separation label, `kallisto/token/v1`, so it can never be reused for
another purpose and produce a value that means something here. The access log derives its own key
under a different label — see [telemetry/README.md](./telemetry/README.md) for why that separation
is load-bearing rather than tidy.

## Lookup is constant-time

A presented token is hashed once and compared against every entry with
`constant_time::verify_slices_are_equal`, without returning early on the first match. Neither the
time taken nor the work done reveals which entry matched, or whether any did.

The loop running to the end rather than returning at the first hit is a property a test cannot
prove — `verification-status.md` says so rather than claiming otherwise.

The key is stored in its prepared HMAC form. `hmac::Key::new` runs the ipad/opad derivation and two
compression functions; doing that per request cost more than the hash it was preparing for. The
authorization path pays one hash per request, and the access log one more.

## Policy paths copy Vault exactly, including the quirk

In KV version 2 the capability to read a secret is written against `secret/data/payment/db`, and
the capability to list it against `secret/metadata/payment/db`, because the API URL carries a
`data/` or `metadata/` segment the user never typed. Everyone who has written a Vault policy has
forgotten this at least once.

Kallisto copies it, so a policy written for a real Vault works here unchanged, and a policy written
here keeps working when somebody outgrows Kallisto and moves to OpenBao.

Three capabilities, because three is all a read-only resolver can mean: `read`, `list`, `deny`. A
Vault policy carrying `create`, `update` or `sudo` still parses — those capabilities simply grant
nothing, which is the truth, since there is no write path for them to authorise.

Rules are compiled when a snapshot is built, not when a request arrives. The pattern text is the
same for the life of a file.

### Where this deliberately differs from Vault

Real Vault resolves a conflict by path specificity: the most specific matching rule wins, so a
narrow `read` grant beats a broad `deny`. Kallisto gives `deny` precedence over every other rule,
however broad.

The two agree on every policy that does not contain a contradiction. Where they disagree, Kallisto
refuses a request Vault would have allowed. That direction is deliberate — a 403 is a visible,
diagnosable failure, and the alternative is serving a secret because a rule twelve lines further up
was worded more precisely.

## A file with no token table

No token key and no tokens is the sidecar deployment: one app, its own file, and the bucket
credential is the boundary. Every read is then permitted, and `sys/health` and
`kallisto_authorization_enforced` both report it so an operator sees it rather than discovers it.

Anything else is default deny, including a request that carried no token.

A file carrying tokens but *no key to check them against* is refused outright. Such a table would
authenticate nobody, and serving it would mean serving with authorization silently switched off.

## Minting and revoking

```bash
kallisto-ctl gen-key                                      # a token_key for the file
kallisto-ctl mint-token --in prod.kal --policy app --policy db
```

`mint-token` prints the token once, together with the line to paste into the plaintext file's
token table. What the file stores is the hash; nothing anywhere can recover the token from it.

Everything that writes a sealed file happens offline in `kallisto-ctl`, never over the network. To
revoke, delete the line and re-seal; every machine picks the change up on its next poll.
