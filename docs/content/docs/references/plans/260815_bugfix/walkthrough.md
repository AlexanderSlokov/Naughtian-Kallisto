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

## Task 3: Adjust Benchmark Scripts for 256K Capacity
- **Keyspace Limit**: Updated `put_bench.js`, `get_bench.js`, `mixed_bench.js`, `seed.js`, and `wrk2_put.lua` to expand the key array ranges (`1000`/`10000` -> `256000`) so they fully exercise the new capacity limit.
- **Seeding Depth**: Adjusted `run_server_bench.sh` to seed via `--iterations 256000` and `run_release_bench.sh` to run wrk2 for `6s` to ensure enough data points are actually seeded.

## Task 4: Address Post-Review Feedback
- **ABA Invariant Comment**: Cập nhật lại trình tự ở code đẩy (eviction). Bây giờ slot bucket trỏ về ô nhớ cũ sẽ được đánh dấu xoá *trước* khi ghi dữ liệu mới lên trên `storage`. Việc này hoàn toàn an toàn do write lock ở shard level, comment ABA Invariant cũng đã được cập nhật làm nổi bật rõ critical section này.
- **CLOCK Eviction (Tham chiếu)**: Đã thay thế `inserted_at_ms` (FIFO) hoàn toàn bằng thuật toán CLOCK (Approximate LRU). Property mới là `referenced: AtomicBool`. Giờ đây, chỉ cần dùng read-lock cũng có thể update `referenced = true` một cách siêu nhẹ (Atomic Ordering Relaxed) mà không làm ngắt các read concurrent.
- **Metric Theo Shard**: Đã bổ sung `pub fn get_shard_stats` cho `ShardedCuckooTable` để lấy độ đo đạc chi tiết theo từng phân mảnh, hỗ trợ đánh giá độ lệch (skewness) của dữ liệu.
- **Concurrent Stress Test**: Đã bổ sung test `test_concurrent_insert_and_read`. Test mô phỏng nhiều thread chèn dữ liệu vượt capacity và nhiều thread đọc ở cường độ cực cao cùng lúc, và **ASSERT** rằng không bao giờ có key bị đọc ra thành giá trị rác hoặc giá trị của người khác. Test chạy thành công và báo xanh.
