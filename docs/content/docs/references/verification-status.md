---
title: "Verification Status"
weight: 11
---

Per-invariant status for ADR-0013. ADR-0013 requires that unproven invariants be
reported as explicitly as proven ones — "an unproven invariant list is as
valuable as the proven list" — and that every invariant test be demonstrably
fail-able. Both are recorded here.

Last reviewed: 2026-09-15.

## How to read this

| Status        | Meaning                                                                                    |
|---------------|--------------------------------------------------------------------------------------------|
| **Proven**    | A fail-able automated check covers the invariant as stated.                                |
| **Qualified** | Covered, but the invariant as written in ADR-0013 is imprecise. The precise form is given. |
| **Partial**   | Covered for part of its stated scope. The uncovered part is named.                         |
| **Unproven**  | No check. The blocker is named.                                                            |

`make verify` is the only blocking gate. `loom`, `fuzz`, `durability` and
`mutants` run on a schedule (`.github/workflows/verification-scheduled.yml`).

## Group A — KV-v2 semantics

Covered by `cargo test -p kallisto_kv_model` (`make verify-proptest`).

The strongest check in this group is **`prop_matches_oracle`**: a differential
property test that runs random operation sequences through `apply` and through
`oracle::Oracle`, an independent reference implementation that shares no code
with it, and requires identical observable state at every step.

| ID                                 | Status    | Check                                                                                                                                                                 | Fails if you                                                                                 |
|------------------------------------|-----------|-----------------------------------------------------------------------------------------------------------------------------------------------------------------------|----------------------------------------------------------------------------------------------|
| A1 monotonic `current_version`     | Proven    | `prop_a1_current_version_monotone`                                                                                                                                    | make `apply_put` reuse or decrement `current_version`                                        |
| A2 versions ascending, no dups     | Proven    | `prop_a2_versions_strictly_ascending`                                                                                                                                 | insert the new version anywhere but the tail                                                 |
| A3 `destroyed` is terminal         | Proven    | `prop_a3_destroyed_is_terminal`, `a3_undelete_on_destroyed_is_a_noop`                                                                                                 | drop the `destroyed` guard in `apply_undelete`                                               |
| A4 CAS mismatch is inert           | Proven    | `prop_a4_cas_mismatch_is_inert`, `prop_a4_cas_required_rejects_missing_cas`                                                                                           | move the CAS check after `new_meta.current_version += 1`, or drop the `cas_required` arm     |
| A5 write yields a readable version | Qualified | `prop_a5_put_yields_a_readable_version`                                                                                                                               | remove `Effect::WriteVersion`, `WriteMeta` or `IndexPath` from the put effects               |
| A6 `undelete ∘ soft_delete = id`   | Qualified | `prop_a6_undelete_inverts_soft_delete`, `prop_a6_undelete_rearms_ttl`                                                                                                 | zero `deletion_time_ms` on undelete when a TTL is configured                                 |
| A7 `versions.len() ≤ max_versions` | Qualified | `prop_a7_max_versions_enforced_after_write`, `prop_a7_retains_newest_window`, `a7_default_limit_applies_when_unset`, `a7_lowering_the_limit_defers_to_the_next_write` | skip destroyed versions while pruning, or treat `max_versions == 0` as "no limit"            |
| A8 destroy keeps `VersionState`    | Proven    | `a8_destroy_retains_version_state`                                                                                                                                    | remove the `VersionState` instead of setting `destroyed`                                     |
| A9 storage keys injective          | Proven    | `prop_a9_storage_keys_are_injective`, `prop_a9_meta_key_roundtrips`, `a9_rejects_pre_length_prefix_keys`                                                              | revert either key builder to `{prefix}:{path}`, or read a path back with a fixed byte offset |

### Where ADR-0013's wording is imprecise

**A7** is stated as "`versions.len() ≤ max_versions` after trimming". That is
only true *after a write*. Lowering `max_versions` through a metadata update does
not prune retroactively; the next write catches up in one pass. A property test
asserting the unqualified form fails on the sequence
`Put, Put, Put, UpdateMeta{max_versions: 1}`. ADR-0013 should be amended to say
"after a write".

**A6** is stated as "`undelete ∘ soft_delete = identity` for non-destroyed
versions". It holds only when `delete_version_after` is disabled. With a TTL
configured, undelete re-arms the timer from the undelete time, so the version
returns with a different `deletion_time_ms` than it had before the soft delete.
Both halves are tested separately.

**A5** is stated as "`put` followed by `read` of that version returns the exact
payload written". The model carries no payload — `KvOp::Put` only records
`payload_len` — so the model-level test covers the metadata half: the version
exists, is readable at write time, and the engine is told to write the payload,
the metadata and the path index. **The payload round-trip itself is covered at
the engine level, not here**, by `tests/e2e_vault_compat.rs`.

## Group B — concurrency

Covered by `make loom` (scheduled). Loom tests the real `LockFreeQueue` through
the `kallisto_queue::sync` shim, not a copy of it.

| ID                                                | Status       | Check                                                                                                                                    | Fails if you                                                                             |
|---------------------------------------------------|--------------|------------------------------------------------------------------------------------------------------------------------------------------|------------------------------------------------------------------------------------------|
| B1 no loss, no double-dequeue                     | Proven       | `b1_item_neither_lost_nor_duplicated`, `b1_concurrent_producers_preserve_every_success`, plus `tests/queue_stress.rs` at real contention | publish `sequence` before writing the slot, or advance `dequeue_pos` without reading     |
| B2 full queue rejects, no overwrite               | Proven       | `b2_full_queue_rejects_without_overwriting`, `b2_slot_handover_is_exact`                                                                 | return `Ok` instead of `Err(Full)` when `dif < 0`, or drop the `dif < 0` branch entirely |
| B3 cuckoo insert→lookup                           | **Unproven** | —                                                                                                                                        | see below                                                                                |
| B4 CLOCK eviction leaves no dangling read         | **Unproven** | —                                                                                                                                        | see below                                                                                |
| B5 `async_worker.join()` before `rocksdb.flush()` | **Unproven** | —                                                                                                                                        | see below                                                                                |

### Why B3/B4 are not loom tests

`CuckooTable` guards all of its state with a single `parking_lot::RwLock`
(`src/engine/cuckoo_table/table.rs:15`). B3 and B4 are therefore *logic*
invariants under a lock, not memory-ordering invariants, and loom is the wrong
instrument: loom cannot model `parking_lot`, and replacing the lock with
`loom::sync::RwLock` would verify a different data structure.

What covers them today: `src/engine/cuckoo_table/tests.rs` and
`src/engine/sharded_cuckoo_table.rs`'s tests, including the fill-to-capacity and
hash-decorrelation regression tests. That is real coverage of the logic, but it is
single-threaded, so **concurrent** insert/lookup/evict interleavings remain
unverified. Closing this properly means either a multi-threaded stress test with
an invariant oracle, or `shuttle` — which ADR-0013 already names as the fallback
for exactly this case.

### Why B5 is not a loom test

Drop order is a sequential property of `impl Drop for KvEngine`, not a
concurrency property: `async_worker.join()` and `rocksdb.flush()` are called in
that order on one thread (`src/engine/kv_engine.rs`). Loom adds nothing. A
regression test that asserts the ordering observably — a worker that records a
completion marker the flush path then requires — is straightforward and is the
right fix. It is not written yet.

## Group C — memory safety

Covered by `make verify-miri` (blocking).

| ID                                          | Status  | Check                                                             | Fails if you                                                              |
|---------------------------------------------|---------|-------------------------------------------------------------------|---------------------------------------------------------------------------|
| C1 `archived_root` triggers no UB           | Partial | `engine::traits::rkyv_safety` under Tree Borrows                  | read past the archived vector's length                                    |
| C2 `unsafe impl Send/Sync` sound            | Proven  | `kallisto_queue::tests::send_across_thread` under Miri            | take the slot pointer from a shared reference instead of the `UnsafeCell` |
| C3 dropping a non-empty queue leaks nothing | Proven  | `kallisto_queue::tests::drop_partially_filled_no_leak` under Miri | remove the drain loop from `impl Drop`                                    |

### C1's scope, stated plainly

What is verified: `archived_root` over archives **this crate produced**, which is
the only case the engine's read path has a contract for.

Two things are *not* verified, and both are recorded rather than hidden:

1. **Corrupted or attacker-chosen bytes.** The engine calls
   `unsafe { rkyv::archived_root::<T>(bytes) }` with no validation, so malformed
   input is undefined behaviour by contract. Closing this needs
   `#[archive(check_bytes)]` plus a validated read path — the same work as the
   rkyv 0.8 migration, since the archived byte layout changes and persisted
   secrets written by 0.7 need a read path or a format version tag. Tracked with
   RUSTSEC-2026-0235, which is ignored in `deny.toml` because its exploit path is
   the *checked* API that Kallisto never calls.
2. **Stacked Borrows.** Under Miri's default model, rkyv 0.7's `ArchivedVec`
   derives a pointer to the vector's elements from a `RelPtr` field, and the
   resulting range lies outside the retagged field — which Stacked Borrows
   rejects. Tree Borrows accepts it, and Miri itself reports Stacked Borrows as
   experimental, so `make verify-miri-rkyv` runs under Tree Borrows.

   This is **not** a Miri exemption: ADR-0013 forbids `#[cfg_attr(miri, ignore)]`
   and none is used anywhere in this workspace. It is a choice of aliasing model,
   declared here, with the residual risk being that if Stacked Borrows becomes
   the accepted model, this pattern needs the rkyv 0.8 migration to stay sound.

## Group D — durability

Covered by `make durability` (scheduled). `tests/integration/test_persistence.sh`
asserts and exits non-zero on failure.

| ID                                   | Status | Check                      | Fails if you                                                                                       |
|--------------------------------------|--------|----------------------------|----------------------------------------------------------------------------------------------------|
| D1 immediate mode survives `kill -9` | Proven | `D1` in the script         | stop propagating `SyncMode::Immediate` to RocksDB's WAL `sync` flag                                |
| D2 batch mode's documented contract  | Proven | `D2a`, `D2b` in the script | make write-behind never flush (D2a), or let a mid-window crash damage an already-durable key (D2b) |

D2 is asserted in the shape the contract actually makes: once the write-behind
window has passed the write **must** be durable (D2a, fail-able), and a crash
inside the window **may** lose the write but must leave the store openable and
previously-durable keys intact (D2b). The test accepts 200 or 404 for the
in-flight key and nothing else.

## Group E — security

Covered by `make verify-security` (blocking).

| ID                                                       | Status       | Check                                       | Fails if you                                                             |
|----------------------------------------------------------|--------------|---------------------------------------------|--------------------------------------------------------------------------|
| E1 no plaintext secrets in `Debug`/`Display`/errors/logs | Proven       | six tests in `tests/security_invariants.rs` | replace `SecretPayload`'s hand-written `Debug` with `#[derive(Debug)]`   |
| E2 token comparison is constant-time                     | **Unproven** | —                                           | blocked: no token authentication exists in this workspace                |
| E3 explicit `deny` overrides `allow`                     | **Unproven** | —                                           | blocked: `components/kallisto_policy` is a 3-line stub with no evaluator |

E1 covers `{:?}`, `{:#?}`, nesting inside a derived `Debug`, every `EngineError`
variant's `Display`, and the absence of any payload-carrying field on
`KeyMetadata`.

E2 and E3 have **no tests at all**, deliberately. ADR-0013 marks Group E as
blocking, so the temptation is to write something that passes — and that is what
was there before: `e2_token_comparison_uses_ct_eq` grepped the source tree, found
nothing, printed a warning and passed; `e3_policy_deny_overrides_allow` had an
empty body with a `TODO`. A test that cannot fail reports a gate that does not
exist, and inflates the mutation score with a target nothing can kill. Both are
now absent and recorded here. They become writable the moment token auth and a
policy evaluator land, and they must land together with them.

## Tiers 2 and 3

| Item                                              | Status                                                                                                                                         |
|---------------------------------------------------|------------------------------------------------------------------------------------------------------------------------------------------------|
| Creusot pilot on `kallisto_kv_model` (V4)         | Not started. The research task — confirm it builds in a container within 2 hours — has not been run. `make prove` prints a notice and exits 0. |
| TLA+ specs for lease invalidation and gossip (V5) | Deferred to 1.2.0 per ADR-0013, before the control/data plane code is written.                                                                 |
| `cargo-mutants` baseline score                    | Not recorded. `make mutants-core` and `make mutants-all` run, but no baseline has been captured, so there is no number to compare against.     |

## Defects found by this verification work

Listed because the point of the exercise is finding these, and because each one
was previously covered by a test that could not fail.

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
