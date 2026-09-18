---
title: "Verification Status"
weight: 11
---

Per-invariant status for ADR-0013. ADR-0013 requires that unproven invariants be
reported as explicitly as proven ones — "an unproven invariant list is as
valuable as the proven list" — and that every invariant test be demonstrably
fail-able. Both are recorded here.

Last reviewed: 2026-09-18.

## How to read this

| Status        | Meaning                                                                                    |
|---------------|--------------------------------------------------------------------------------------------|
| **Proven**    | A fail-able automated check covers the invariant as stated.                                |
| **Qualified** | Covered, but the invariant as written in ADR-0013 is imprecise. The precise form is given. |
| **Partial**   | Covered for part of its stated scope. The uncovered part is named.                         |
| **Unproven**  | No check. The blocker is named.                                                            |

| **Retired**   | The code the invariant constrained no longer exists. What replaced it is named. |

`make verify` is the only blocking gate. `loom`, `fuzz` and `mutants` run on a
schedule (`.github/workflows/verification-scheduled.yml`).

> **ADR-0015 deleted a large part of what this document covered.** The storage
> engine, the RocksDB backend, the cuckoo cache and the KV-v2 *write* model are
> gone, and with them Group A, half of Group B, part of Group C, and all of
> Group D. Those sections are kept rather than deleted, marked **Retired**,
> because "this was proven and then the code went away" and "this was never
> proven" are different facts and a reader deserves to tell them apart. What
> replaced the behavioural half of them is `make duck`: three real Vault SDKs
> driving the real server.

## Group A — KV-v2 semantics — **Retired**

Covered by `cargo test -p kallisto_kv_model`, which no longer exists.

Every invariant in this group (A1–A9) constrained the KV-v2 **write** path:
version monotonicity, CAS, soft-delete, undelete, destroy, retention windows.
ADR-0015 D1 made every one of those operations answer `403 permission denied`,
and `components/kallisto_kv_model` — including `oracle::Oracle`, the independent
reference implementation the differential test ran against — was deleted with
them. Nine invariants went from Proven to irrelevant in one commit.

This is recorded rather than quietly dropped because the work was not wasted and
the reasoning should survive it: a differential property test against a
separately written oracle is the strongest check in this repository's history,
and if a write path ever returns, this is the shape it should be verified in.

The behavioural gate that replaced it is **`make duck`**, which is a different
kind of check — three real Vault SDKs against the real server, testing
compatibility rather than semantics, because compatibility is what is left to
get wrong.

## Group B — concurrency

Covered by `make loom` (scheduled). Loom tests the real `LockFreeQueue` through
the `kallisto_queue::sync` shim, not a copy of it.

| ID                                                | Status       | Check                                                                                                                                    | Fails if you                                                                             |
|---------------------------------------------------|--------------|------------------------------------------------------------------------------------------------------------------------------------------|------------------------------------------------------------------------------------------|
| B1 no loss, no double-dequeue                     | Proven       | `b1_item_neither_lost_nor_duplicated`, `b1_concurrent_producers_preserve_every_success`, plus `tests/queue_stress.rs` at real contention | publish `sequence` before writing the slot, or advance `dequeue_pos` without reading     |
| B2 full queue rejects, no overwrite               | Proven       | `b2_full_queue_rejects_without_overwriting`, `b2_slot_handover_is_exact`                                                                 | return `Ok` instead of `Err(Full)` when `dif < 0`, or drop the `dif < 0` branch entirely |
| B3 cuckoo insert→lookup                           | **Retired**  | —                                                                                                                                        | see below                                                                                |
| B4 CLOCK eviction leaves no dangling read         | **Retired**  | —                                                                                                                                        | see below                                                                                |
| B5 `async_worker.join()` before `rocksdb.flush()` | **Retired**  | —                                                                                                                                        | see below                                                                                |

### B3, B4 and B5 went with the code

All three constrained the storage engine: the cuckoo cache's concurrent
insert/lookup/evict interleavings, and `KvEngine`'s drop order around RocksDB.
ADR-0015 deleted `src/engine/` and `src/storage/` entirely.

They were **Unproven** when they were deleted, and that is the honest way to
record it: they are not a debt that was paid, they are a debt that was
cancelled. The reasoning that kept them unproven is still worth knowing, because
it applies to any future cache: `CuckooTable` guarded its state with a single
`parking_lot::RwLock`, which made B3 and B4 *logic* invariants under a lock
rather than memory-ordering invariants — loom is the wrong instrument for those,
since it cannot model `parking_lot`, and swapping in `loom::sync::RwLock` would
verify a different data structure. `shuttle`, which ADR-0013 already names as the
fallback for exactly this case, remains the right answer if one is needed again.

B1 and B2 are untouched: `kallisto_queue` survives, and ADR-0015 D15 gave it a
second job draining the access log — this time with several real producers, which
is the first use of the queue's MPMC half for its actual purpose.

## Group C — memory safety

Covered by `make verify-miri` (blocking).

| ID                                          | Status  | Check                                                             | Fails if you                                                              |
|---------------------------------------------|---------|-------------------------------------------------------------------|---------------------------------------------------------------------------|
| C1 `archived_root` triggers no UB           | **Retired** | —                                                             | see below                                                                 |
| C2 `unsafe impl Send/Sync` sound            | Proven  | `kallisto_queue::tests::send_across_thread` under Miri            | take the slot pointer from a shared reference instead of the `UnsafeCell` |
| C3 dropping a non-empty queue leaks nothing | Proven  | `kallisto_queue::tests::drop_partially_filled_no_leak` under Miri | remove the drain loop from `impl Drop`                                    |

### C1 was closed by deletion, which is the outcome ADR-0015 D12 predicted

`rkyv` is gone. The invariant was **Partial**: verified for archives this crate
produced, unverified for corrupted or attacker-chosen bytes, because the engine
called `unsafe { rkyv::archived_root::<T>(bytes) }` with no validation and
malformed input was undefined behaviour by contract. Closing it properly meant
`#[archive(check_bytes)]` and the rkyv 0.8 migration — and because archives were
already on disk, that was a **data migration**, not a dependency bump.

ADR-0015 D12 argued the dependency was not worth its cost. Deleting the storage
engine turned a data migration into a line removed from `deny.toml`:
`RUSTSEC-2026-0235` and `RUSTSEC-2025-0141` are both gone, and the `ignore` list
is now empty for the first time in this project's history.

The second half of the old note is worth keeping as a record of method: the rkyv
test ran under **Tree Borrows** rather than Miri's default Stacked Borrows,
because rkyv 0.7's `ArchivedVec` derived an element pointer from a `RelPtr`
field in a way Stacked Borrows rejects. That was declared as a choice of
aliasing model, not an exemption — ADR-0013 forbids `#[cfg_attr(miri, ignore)]`
and none is used anywhere in this workspace, then or now.

C2 and C3 are untouched. `kallisto_queue` is the only `unsafe` left that Miri
has anything to say about, and `make verify-miri` is now exactly that one crate.

## Group D — durability — **Retired**

Nothing is written any more, so there is nothing to lose.

| ID                                   | Status      | Check | Fails if you |
|--------------------------------------|-------------|-------|--------------|
| D1 immediate mode survives `kill -9` | **Retired** | —     | —            |
| D2 batch mode's documented contract  | **Retired** | —     | —            |

Both were **Proven**, by `tests/integration/test_persistence.sh` under a real
`kill -9`, and both constrained RocksDB's WAL sync behaviour behind the
`/admin/mode/{immediate,batch}` switch. ADR-0015 removed the write path, the
admin API and RocksDB; the script, the `make durability` target and the
scheduled CI job are deleted.

What took its place is not a durability property at all but an *availability*
one, and it is worth naming because it is the question an operator actually has
now: the resolver keeps an encrypted copy of the file on local disk and serves
from it when the bucket is unreachable (ADR-0015 D5). That is covered by the
refresh loop's tests and by `make duck`'s bucket-down case, not here.

## Group E — security

Covered by `make verify-security` (blocking).

| ID                                                       | Status | Check                                                                  | Fails if you                                                                          |
|----------------------------------------------------------|--------|------------------------------------------------------------------------|---------------------------------------------------------------------------------------|
| E1 no plaintext secrets in `Debug`/`Display`/errors/logs | Proven | six `e1_*` tests in `tests/security_invariants.rs`                        | replace `Contents`' or `TokenKey`'s hand-written `Debug` with `#[derive(Debug)]`      |
| E2 token comparison is constant-time                     | Proven | three `e2_*` tests, against `policy_engine::TokenTable::lookup`          | compare a prefix, suffix or truncation of the hash; store a bare digest of the token   |
| E3 explicit `deny` overrides `allow`                     | Proven | two `e3_*` tests, against `RuleSet::allows` and `Snapshot::permits`      | let a grant win over a `deny` that matches the same path, in either written order      |
| D15 nothing in the code is named an audit log            | Proven | `d15_nothing_in_the_code_is_named_audit`                                 | name a file, type, field or config key `audit` anywhere outside a comment              |
| D15 log identifiers are keyed and domain-separated       | Proven | `d15_path_identifiers_cannot_be_used_against_the_token_table`            | hash a path under the token label, or with the token key directly                      |
| D15 no log line carries cleartext                        | Proven | `d15_a_rendered_log_line_carries_no_cleartext`                           | write a path, token or value into a line instead of its identifier                     |

E1 covers `{:?}`, `{:#?}`, nesting inside a derived `Debug`, every error type on
the read path that can reach a log line, every type holding key material, and a
live `Snapshot` — both its rendering and the bytes it actually stores.

**E1 was rewritten when the storage engine was deleted, and the rewrite is the
part worth reading.** Its subjects used to be `SecretPayload`, `KeyMetadata` and
`EngineError`: types belonging to a write path that no longer exists. Deleting
them would have quietly taken E1's six tests with them and left the invariant
marked Proven by nothing. The invariant did not change — its subjects did, and
they are now the types a secret actually passes through. All four redaction
tests were confirmed by replacing a hand-written `Debug` with a derived one and
watching them fail.

**One boundary, asserted rather than left to be found.** A *type* error from the
configuration parser does quote the offending value: `invalid type: string
"...", expected usize`. That is the single place in this program where an error
message echoes its input, and it is deliberate. The configuration file is not
secret-bearing by design (ADR-0003: the seal key and bucket credentials have no
field to go in, and `deny_unknown_fields` means inventing one fails), while
"line 4 is wrong" with no reason costs an operator an hour. The sealed *secrets*
file gets the opposite treatment — `kallisto-ctl` withholds serde's message
there precisely because the value it choked on is a secret.
`e1_configuration_type_errors_quote_the_value_and_that_is_on_purpose` pins the
trade so it cannot be mistaken for an oversight, and has to be the first test
changed if a credential ever gains a config field.

E2 and E3 were blocked until the features they constrain existed. They landed
with `components/kallisto_policy` in M4 of the duck plan, and the tests landed
with them, as ADR-0013 required.

**What E2 proves, and what it does not.** `e2_token_comparison_does_not_match_on_a_partial_hash`
builds a table containing nothing but near misses — the real token's hash with a
single bit flipped, at five positions across its width — and asserts the real
token matches none of them. That kills a prefix compare, a suffix compare and a
truncated compare. `e2_the_stored_form_of_a_token_is_keyed` kills a bare
SHA-256 of the token, which would make the file's plaintext a dictionary attack
away from every token in the fleet.

The remaining half of the property — that `lookup` visits *every* entry rather
than returning at the first hit — is **not** mechanically tested, and saying so
here is the point. No assertion over return values can distinguish an early
return from a full scan, and a timing assertion over a handful of table entries
would measure CI noise. It is held by the implementation, which accumulates into
a local and returns after the loop, and by review. If that loop ever grows a
`break` or a `return` inside it, no test in this repository will notice.

An earlier version of the E2 test asserted that three tokens at three positions
in the table each resolved correctly. A deliberately broken implementation
comparing only the first four bytes of the hash **passed it**. That is recorded
because it is the same failure ADR-0013 was written about: asserting that the
right answers come back does not constrain how the answer is reached. Every
`e2_*` and `e3_*` test here was checked by breaking the implementation on
purpose and confirming it failed.

### ADR-0015 D15, the log path

These are not ADR-0013 Group E invariants, but they live in the same file and
run under the same blocking target, because they are the same kind of property:
a constraint that decays silently if nothing checks it.

**The naming gate earned itself immediately.** `d15_nothing_in_the_code_is_named_audit`
scans `src/`, `components/*/src/`, `cmd/*/src/` and every `.yaml`/`.toml`, and
fails on the word outside a comment. On its first run it failed — on the
`description` field of the telemetry crate's own `Cargo.toml`, written minutes
earlier in the same session, reading "Not an audit log". The constraint is that
the word must not *name* anything; a machine-readable field describing the crate
that way is exactly the surface D15 is worried about, and prose alone would not
have caught it.

**What the domain-separation test protects.** Log path identifiers are keyed
with `HMAC(token_key, "kallisto/log/v1\0")`, not with the token key under the
token label. Paths are chosen by the caller, so a shared PRF would turn the
access log into an oracle emitting `HMAC(token_key, arbitrary string)` — the
material for a reverse table against the file's own token column. The test
hashes the same text both ways and asserts they disagree; it was confirmed by
substituting the forbidden simplification and watching it fail.

**A test that survived its mutation, and what fixed it.** The first version of
`the_logged_path_identifier_matches_the_key_the_file_carries` obtained its
expected value by calling `Snapshot::path_id` — the function under test. Making
the precomputed identifier hash the wrong string moved both sides together and
the test stayed green. It now derives the key from the token key directly, and
kills that mutation. This is the second time in this plan the same mistake has
been caught (see the E2 note above); both are recorded because the pattern —
comparing an implementation against itself — is not visible from a passing test
run.

**What is not proven here.** That the access log records *every* request is
enforced structurally, by an axum layer rather than a call in each handler, and
asserted for the routes that exist today
(`every_route_produces_exactly_one_line`). A future route added outside that
layer would not be caught by any test, in the same way E2's full-scan property
is not.

## Tiers 2 and 3

| Item                                              | Status                                                                                                                                         |
|---------------------------------------------------|------------------------------------------------------------------------------------------------------------------------------------------------|
| Creusot pilot on `kallisto_kv_model` (V4)         | **Moot.** The crate it was to be piloted on is deleted (ADR-0015 D1). `make prove` prints a notice and exits 0.                                |
| TLA+ specs for lease invalidation and gossip (V5) | **Moot.** There are no leases and no gossip: ADR-0015 D16 removed the control plane and D8 made tokens a table in a file rather than state.    |
| `cargo-mutants` baseline score                    | Not recorded. `make mutants-core` and `make mutants-all` run, but no baseline has been captured, so there is no number to compare against.     |

## Defects found by this verification work

Listed because the point of the exercise is finding these, and because each one
was previously covered by a test that could not fail.

The first eleven were found in the engine era. They are kept after that code's
deletion for one reason: they are the evidence for *why* ADR-0013 requires every
invariant test to be demonstrably fail-able, and deleting the evidence along with
the code would leave the rule looking like a preference. The rows below them are
from the resolver.

| Found by        | Defect                                                                                                                                                                                                                                                                                                |
|-----------------|-------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| Miri            | `LockFreeQueue::enqueue` wrote through a pointer derived from a shared reference — UB under Stacked Borrows. Fixed with `UnsafeCell`.                                                                                                                                                                 |
| Loom            | `new(1)` built a queue that silently overwrote the unconsumed item and then spun forever in `dequeue`: capacity 1 collapses the "filled" (`pos + 1`) and "drained" (`pos + capacity`) sequence markers. Now rejected at construction.                                                                 |
| Loom            | `enqueue`/`dequeue` discarded the value a failed `compare_exchange_weak` returns and retried against a stale `pos`, spinning until the winner published its `sequence` store. Bounded on real hardware, unbounded if the winner is preempted in that window. Fixed by adopting the reported position. |
| Property test   | Pruning skipped destroyed versions, so a write with `max_versions` reached could prune *the version it had just written* — metadata ended with `current_version` pointing at a version not present, and the following read returned `InvalidVersion`.                                                 |
| Property test   | `cas_required` was stored and never enforced: a write with no `cas` succeeded against a key that required one.                                                                                                                                                                                        |
| Property test   | `delete_version_after` was stored and never applied.                                                                                                                                                                                                                                                  |
| Property test   | Soft-delete mutated a destroyed version; undelete on a destroyed version returned an error instead of a no-op.                                                                                                                                                                                        |
| A9 review       | The path index rebuild read paths back with a fixed `key[2..]` offset after the keys became length-prefixed, indexing `00000006:app/db` as a secret path and breaking `LIST` after every restart.                                                                                                     |
| Durability test | The server ignored `--db-path` and deleted its storage directory on every startup, so nothing persisted across a restart. The benchmark scripts had been passing `--workers` and `--http-port` all along, equally ignored.                                                                            |
| Durability test | `SyncMode::Immediate` never enabled RocksDB's WAL `sync`, so a write that answered 2xx did not survive `kill -9`. D1's guarantee was unimplemented.                                                                                                                                                   |
| Durability test | `POST /admin/mode/immediate` and `/admin/mode/batch` returned `"OK"` without changing the mode, which made immediate mode unreachable from the API.                                                                                                                                                   |
| Mutation check  | The first E2 test asserted that three tokens at three table positions each resolved correctly. An implementation comparing only the first four bytes of the hash **passed it**. Replaced with a table of near misses — the real hash with one bit flipped at five positions — which kills prefix, suffix and truncation compares. |
| Mutation check  | The first test for the access log's path identifier obtained its expected value by calling `Snapshot::path_id`, the function under test. Making the precomputed identifier hash the wrong string moved both sides together and the test stayed green. It now derives the key from the token key directly. |
| Building M5     | `open()` returned an owned `Contents`, so `serde_json` built a second copy of every secret as `String`/`Value` on the heap that nothing zeroized — surviving in freed memory, which is exactly what a core dump or a swap file picks up. The in-RAM barrier would have been decorative. `open()` now returns a borrowed view into the self-wiping buffer. |
| Building M6     | `TokenKey::hash` rebuilt its `hmac::Key` on every call, so every request had been paying for the ipad/opad derivation since M4. |
| Building M6     | A disabled access log still enqueued, so with no writer draining it the queue filled and every line counted as *dropped* — switching the log off would have raised the alarm that means "something is flooding this process". |
| D15 naming gate | On its first run the new scan failed on the telemetry crate's own `Cargo.toml` description, written minutes earlier in the same session, reading "Not an audit log". |
| Building M7     | ADR-0015 D8's token table had no way for an operator to produce a valid row: the keyed hash prefixes a label, which `openssl dgst -hmac` cannot do. The feature had been unusable since M4. `kallisto-ctl mint-token` closes it. |
| Benchmarking    | M5 measured *faster* than M3 when the two were measured one after the other — the laptop warming up, not a result. Interleaving both binaries in one loop showed them equal. Every comparison since is interleaved. |
| Self-review     | An external memory scan being refused was attributed to `prctl(PR_SET_DUMPABLE, 0)`. Two processes differing only in that call had identical `/proc/<pid>` ownership; the actual blocker was the machine's Yama `ptrace_scope`. The claim was retracted in the plan and in the code's doc comment. |
