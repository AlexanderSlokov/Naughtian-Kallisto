# Recommended patterns

## Point your existing Vault SDK at it

```diff
- VAULT_ADDR=https://vault.internal:8200
+ VAULT_ADDR=http://127.0.0.1:8200
```

That is the whole integration. An unmodified SDK asks `sys/health` and `auth/token/lookup-self`
before it asks for anything useful, and both are implemented for exactly that reason (ADR-0015 D7).
Leaving them out is how a compatibility layer fails on the first line of somebody's `main()`.

Backing out is the same line, which is the property worth protecting: nothing in your application
should encode that Kallisto is there.

## Fetch per use, discard immediately

The pattern the read path was rebuilt for (ADR-0016). Fetch a secret right before you use it, use
it, drop it — no caching in the application.

It looks wasteful and it is not, on either axis. It narrows the window in which the secret sits in
your process's memory to the smallest it can be, which matches the threat model Kallisto is built
around; and the read is a localhost round trip to a process holding the value in RAM, not a network
call to a central Vault.

This is why the read path is treated as a hot path at all. See
[serving-kv-secrets.md](../../explanation/why-use-naughtian-kallisto/serving-kv-secrets.md) for
what it measures.

If your application instead reads its secrets once at startup and holds them, that works too — it
is what ADR-0015 originally assumed. Set `workers: 1` and spend nothing on the read path.

## One file per trust boundary

The sidecar shape of ADR-0015 D8: one app, its own sealed file, and the bucket credential is the
boundary. The file then needs no token table, every read is permitted, and there is one less thing
to rotate.

Reach for the token table when several applications share a file and must not read each other's
secrets. Splitting the file is usually the cheaper answer — it is one more object in a bucket, and
it makes the blast radius of a stolen bucket credential legible.

## Alert on the file version, not just liveness

`sys/health` returns 200 when a file is loaded and 503 when none is, so an existing liveness probe
keeps working untouched. That is deliberate but it is not enough on its own: a machine serving a
file from last Tuesday is healthy by that definition.

Scrape `kallisto_file_version` across the fleet and alert when they disagree, and on
`kallisto_refresh_failures_total` rising. See
[telemetry/key-metrics.md](./telemetry/key-metrics.md).

## Rotate by re-sealing, not by endpoint

There is no rotation API, because there is no write path. Rotating a secret, revoking a token and
changing a policy are the same motion: edit the plaintext, `kallisto-ctl seal`, upload. Every
machine picks it up on its next poll.

Content versions are monotonic and never reused. A file older than the one a server already holds
is refused — the attacker in the threat model is whoever can write to the bucket, so bucket
versioning is under their control and only the in-process check helps.

## Keep the port loopback

The default, and refused otherwise at startup unless you pass `--i-accept-the-risk`. Kallisto has
no network authentication story: the token table authorises *which secrets* a caller may read, not
*who may reach the port*. Binding it to a routable address turns a sidecar into an unauthenticated
secrets endpoint.

If you want the port reachable from another container in the same pod, share a network namespace
rather than changing the bind address.
