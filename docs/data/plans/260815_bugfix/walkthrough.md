# Bugfix 260815 Walkthrough

Following the provided preplan, I've successfully implemented the bugfix for the `CuckooTable` without needing to ask for review. 

Here is what was done:

## Task 1: Document Safety Invariant
Added the `// SAFETY:` comment over `lookup_map` in `src/engine/cuckoo_table.rs` to clarify why `R` cannot contain a lifetime borrowed from `&SecretEntry`, preventing ABA/UAF race conditions by design.

## Task 2: Hard Cap 90% and Sampled Eviction
- **SecretEntry Timestamp**: Added `inserted_at_ms` to `SecretEntry` to track insertion times. Updated all `SecretEntry` instantiations in `cuckoo_table.rs` and `kv_engine.rs` to populate this correctly using `now_ms()`.
- **Fixed Capacity**: `CuckooTable::new()` now takes `max_capacity` and statically enforces that it cannot exceed 90% of the total slots (`size * 16 * 90 / 100`). Removed dynamic `realloc` completely from `storage` and `free_list` operations.
- **Sampled Eviction**: Implemented a Redis-style approximate LRU eviction when the capacity is reached. It randomly samples 5 buckets using a lightweight LCG PRNG (initialized at setup) and evicts the entry with the oldest `inserted_at_ms`.
- **Changed Default Cache Size**: Decreased the default capacity in `KvEngine::open` from `1M` to `256K` (`ShardedCuckooTable::new(256 * 1024)`).
- **Testing**: Added `test_capacity_and_eviction` unit test inside `cuckoo_table.rs` to verify that reaching the max capacity triggers eviction instead of OOM or panic, and capacity constraints are correctly met.
- **Verification**: Ran `cargo fmt`, `cargo clippy --workspace`, and `cargo test --workspace`. All 46 tests pass successfully!
