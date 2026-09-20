## ABA slab-index reuse exposing a post-unlock zero-copy deserialize race — CWE-416 (Use-After-Free) + CWE-362 (TOCTOU race)

Lo ngại reader tra cuckoo-table lấy index N vào arena, nhả read-lock của shard, rồi writer tái sử dụng ô N (từ
free-list) và ghi đè giá trị mới; reader sau đó mới zero-copy deserialize (rkyv::archived_root) trên byte đã bị ghi dở →
hoặc lỗi deserialize (~1%), hoặc tệ hơn là trả nhầm secret của key khác một cách im lặng (HTTP 200, không log, không
lỗi). Đề xuất vá: thêm generation counter cho index (giải ABA), và copy-out bytes trước khi diễn giải rkyv (giải
torn-read).

### Result: OBJECTION

Không tái hiện được trong code hiện tại. Lý do (đã kiểm chứng ở src/engine/cuckoo_table.rs + src/engine/kv_engine.rs):

- CuckooTable::lookup_map giữ RwLockReadGuard sống suốt lúc closure f chạy — return Some (f (se)) khiến f (se) chạy xong
  trước khi guard bị drop, không phải sau.
- Mọi closure thật sự dùng (read_raw_optimistic, read_version, read_metadata) đều diễn giải rkyv và copy ra owned data
  (.to_string (), field Copy) ngay bên trong closure đó — không có bước nào giữ reference vượt ra khỏi lock.
- Chữ ký F: FnOnce (&SecretEntry) -> R bị Rust desugar thành higher-ranked for<'a> FnOnce (&'a SecretEntry) -> R, nên về
  mặt compile-time R không thể mang theo reference mượn từ arena — bug này không viết ra được qua API hiện có nếu không
  dùng unsafe/transmute lifetime, và không có chỗ nào làm vậy.

## Giảm sức chứa tối đa xuống 256k slot (2^n=256k cho hai bảng cuckoo-table)

Đuổi theo mẫu ngẫu nhiên. Không cần cấu trúc dữ liệu mới. Khi chạm 90%: bốc ngẫu nhiên 5 bucket, trong đó chọn entry có
timestamp cũ nhất, đuổi nó, ghi cái mới vào. Đây chính xác là cách Redis làm — LRU của Redis là xấp xỉ bằng lấy mẫu, mặc
định 5 mẫu, không phải LRU thật.

Kèm theo: hạ mặc định xuống 256K entry.

### Result: CONFIRMED.

CuckooTable::insert khi free-list rỗng luôn realloc nhân đôi storage, không hề có cap, chỉ panic!("OOM...") khi
allocator thật sự hết bộ nhớ (cuckoo_table.rs:200-226). Giới hạn 256 lần displacement chỉ áp cho việc tìm slot trống
trong bucket, không chặn storage phình vô hạn. Đây là gap thật — CWE-770 (unbounded resource allocation) — và hướng sửa
(hạ default 256K + sampled eviction kiểu Redis) là hợp lý.

### Xác nhận thêm: không có hard cap 90% nào đang tồn tại

Grep toàn bộ `src/engine/cuckoo_table.rs`, `sharded_cuckoo_table.rs`, `kv_engine.rs` cho `0.9`, `90%`, `load_factor`,
`max_load`: không có kết quả nào ngoài `storage_capacity * 2` (realloc) và `free_list_capacity * 2` (realloc). Không hề
có bước tính load factor rồi so với ngưỡng trước khi insert. Cơ chế "full" duy nhất hiện có là thất bại sau đúng 256
lần displacement (`for i in 0..256`, cuckoo_table.rs:232), tức là hệ quả gián tiếp của tải cao chứ không phải một guard
90% được tính trước — **xác nhận: chưa có hard limit 90% nào cho hai bảng cuckoo (table_1/table_2) hay cho storage
arena.**

Yêu cầu chốt lại cho lần sửa này (từ trao đổi trong session):

- Toán: bucket 8-way, mô hình cuckoo *không rehash* lý thuyết bão hoà quanh ~95% load factor trước khi chuỗi
  displacement không còn tìm được chỗ trống. Khoá cứng ngưỡng vận hành ở **90%** để có biên an toàn trước khi chạm trần
  lý thuyết.
- Khi đầy (chạm 90%): **từ chối ghi thêm**, không rehash, không cấp phát thêm bộ nhớ (bỏ toàn bộ hai đường `realloc`
  hiện tại cho `storage` và `free_list`), không dịch chuyển Vector Arena. Toàn bộ dung lượng phải cấp phát cố định một
  lần lúc khởi tạo.
- Ghi chú phụ: `src/storage/cache.rs::Cache` (DashMap-based) đã có sẵn kiểu "bounded capacity, reject khi đầy" tương tự
  tinh thần này (xem doc comment của nó, tự nhận là ứng viên thay thế `ShardedCuckooTable`), nhưng **không được wire
  vào `KvEngine` ở đâu cả** (grep không thấy `Cache::new`/`storage::cache` được dùng ngoài file của chính nó) — hiện là
  dead code, cache đang chạy thật trong request path vẫn là `ShardedCuckooTable`. Không nhầm hai cái này khi sửa.

## Kế hoạch khắc phục — giao cho Gemini

Bối cảnh: Task 1 (ABA/UAF race qua zero-copy deserialize) đã bị bác bỏ bằng chứng cứ đọc code — **không cần sửa gì**,
chỉ cần giữ nguyên invariant hiện có. Task 2 (thiếu hard cap dung lượng) đã được xác nhận có thật — **đây là việc cần
làm**. Plan dưới đây chỉ dành cho task 2.

### Task 1 — Không sửa code, chỉ chốt invariant bằng comment

Mục tiêu: ngăn người sau vô tình phá vỡ property "closure của `lookup_map` không được trả reference mượn từ arena" mà
hiện đang được Rust's higher-ranked trait bound (`for<'a> FnOnce(&'a SecretEntry) -> R`) đảm bảo miễn phí ở compile
time.

- File: `src/engine/cuckoo_table.rs`, ngay trên `pub fn lookup_map`.
- Thêm một dòng `// SAFETY:`/doc-comment giải thích: *"R không được mang lifetime mượn từ `&SecretEntry` — đây là bất
  biến bảo vệ khỏi ABA/UAF race khi slot được tái sử dụng qua free-list. Đừng thêm API nào trả `&SecretEntry`/`&[u8]`
  ra ngoài closure này."*
- Không đổi logic, không đổi signature. Chỉ 1 comment. Không cần test mới cho việc này (không có behavior thay đổi).

### Task 2 — Fixed-capacity CuckooTable, hard cap 90%, sampled eviction, bỏ realloc

**File chính: `src/engine/cuckoo_table.rs`**

1. `SecretEntry` (đầu file): thêm field timestamp phục vụ eviction, ví dụ `pub inserted_at_ms: u64` (dùng cho "oldest
   timestamp" khi sample). Cập nhật `Default` impl tương ứng. Rà lại toàn bộ nơi khởi tạo `SecretEntry { key, payload }`
   theo kiểu struct-literal (cuckoo_table.rs, kv_engine.rs) để set field mới — không dùng `..Default::default()` một
   cách qua loa nếu timestamp cần giá trị thật tại thời điểm ghi (dùng hàm `now_ms()` đã có sẵn trong `kv_engine.rs`,
   hoặc thêm helper tương đương trong `cuckoo_table.rs` nếu cần tránh phụ thuộc chéo module).

2. `UnsafeCuckoo::new` / `CuckooTable::new`: đổi từ mô hình "cấp phát ban đầu rồi realloc khi đầy" sang **cấp phát cố
   định đúng 1 lần** ở dung lượng tối đa. Tính rõ: dung lượng tối đa storage = 90% × (bucket_count × 8 × 2 bảng). Bỏ
   tham số `initial_capacity` mang ý nghĩa "sẽ lớn thêm" — đổi tên/ý nghĩa param thành capacity cố định (vd
   `max_capacity`) để không gây hiểu lầm cho người đọc sau.

3. `CuckooTable::insert`: xoá hai nhánh `realloc` (dòng ~208-219 cho `storage`, ~339-345 và ~368-374 cho
   `free_list` trong `remove`). Free-list vẫn cần cấp phát cố định đủ lớn = `max_capacity` (worst case mọi slot đều bị
   remove), không cần grow động nữa.

4. Thêm logic ngưỡng 90% trước khi lấy slot mới (khi `free_list_size == 0` và cần slot mới, tức nhánh hiện đang gọi
   `realloc`):
   - Nếu `storage_size < 90% * max_capacity`: cấp slot mới bình thường như cũ (không realloc nữa, chỉ index tăng dần
     trong vùng đã cấp phát sẵn).
   - Nếu đã chạm ngưỡng 90%: chạy **sampled eviction kiểu Redis approximate-LRU** — bốc ngẫu nhiên 5 bucket (dùng
     `rand`/`fastrand` hoặc một PRNG nhẹ đã có sẵn trong workspace deps — kiểm tra `Cargo.lock` trước khi thêm dependency
     mới), trong các slot hợp lệ của 5 bucket đó chọn entry có `inserted_at_ms` nhỏ nhất, evict nó (trả index về free
     list ngay lập tức rồi dùng chính index đó cho entry mới — không cần đi qua free_list làm gì nếu evict-and-reuse
     trực tiếp được).
   - Nếu evict xong mà (trường hợp hiếm, race giữa các shard hoặc bucket rỗng) vẫn không tìm được nạn nhân: giữ hành
     vi "reject" hiện có, nhưng đổi `eprintln!` sang logging có cấu trúc (xem AGENTS.md § Logging — JSON cho
     debugging/observability) và cân nhắc đổi `insert()` từ trả `bool` sang `Result<InsertOutcome, EngineError>`
     hoặc enum tương tự để caller phân biệt được "đã evict để chèn" vs "bị từ chối vì đầy" — hiện tại `KvEngine`
     đang bỏ qua hoàn toàn giá trị trả về của `cache.insert(...)` (xem `kv_engine.rs:100, 293, 300, 329, 359, 390`),
     nên ít nhất log lại khi bị reject để có observability.

5. **`src/engine/kv_engine.rs:42`**: đổi default `ShardedCuckooTable::new(1024 * 1024)` (~1M) thành
   `ShardedCuckooTable::new(256 * 1024)` (256K) như user yêu cầu.

6. **`src/engine/sharded_cuckoo_table.rs`**: đảm bảo việc chia 64 shard vẫn hoạt động đúng với capacity mới (256K/64 =
   4096 item/shard) — kiểm tra lại nhánh `if buckets_per_shard < 64 { buckets_per_shard = 64; }` (dòng 15) không bị
   vênh với ngưỡng 90% mới khi số shard × capacity/shard không chia hết đẹp.

**Không đụng vào `src/storage/cache.rs`** — file đó hiện không nằm trong request path (dead code), không phải phạm vi
của lần sửa này; nếu Gemini thấy tiện lợi copy logic bounded-capacity từ đó thì được, nhưng đừng tưởng sửa file đó là
sửa xong bug — sai file thì không có tác dụng gì trên production path.

**Test bắt buộc** (theo AGENTS.md § Tests — "Every new function gets a test. Bug fixes get a regression test."):
- Test nạp đầy cache tới đúng ngưỡng 90%, xác nhận insert tiếp theo kích hoạt eviction thay vì OOM/panic.
- Test xác nhận sau khi eviction, entry cũ nhất (theo `inserted_at_ms`) trong 5 sample bị mất, các entry khác trong
  cùng bucket không bị đụng tới.
- Test xác nhận **không còn lệnh `realloc`** nào chạy được khi liên tục insert vượt quá capacity ban đầu (có thể assert
  qua `get_memory_stats().storage_capacity` giữ nguyên không đổi trước/sau khi vượt ngưỡng 90%).
- Chạy `make test` + `cargo clippy --workspace` + `make format` trước khi coi là xong, theo đúng AGENTS.md.

## Đã làm: dọn dead code `Cache` (DashMap) — KHÔNG PHẢI task 2, đã xong trong session này

Ngoài lề so với task 2 nhưng phát hiện trong lúc điều tra: `src/storage/cache.rs::Cache` (DashMap-based, đã có sẵn
kiểu bounded-capacity + reject-khi-đầy) **không được wire vào `KvEngine`** ở đâu cả — cache chạy thật trong request
path production vẫn luôn là `ShardedCuckooTable`. Nơi duy nhất còn đụng tới `Cache` là
`benchmarks/storage/storage_bench.rs`, và file bench đó **cũng không hề được Cargo build**: `Cargo.toml` khai sai khoá
`[[benchmarks.storage]]`/`[[benchmarks.security]]` (không phải cú pháp Cargo hợp lệ — đúng phải là `[[bench]]`), nên
Cargo lặng lẽ bỏ qua cả hai benchmark này bấy lâu nay (`cargo check --benches` không compile chúng, chỉ warning "unused
manifest key: benchmarks").

Đã xác nhận với user và **xoá hẳn**:

- Xoá `src/storage/cache.rs`; bỏ `pub mod cache;` khỏi `src/storage/mod.rs`.
- `benchmarks/storage/storage_bench.rs`: bỏ hàm `bench_cache_operations`; viết lại `bench_mixed_workload` để dùng
  `ShardedCuckooTable` + `SecretEntry` (cache thật) thay vì `Cache` đã xoá.
- `Cargo.toml`: sửa `[[benchmarks.storage]]`/`[[benchmarks.security]]` → `[[bench]]` đúng cú pháp — giờ
  `cargo bench --bench storage_bench`/`security_bench` mới thực sự compile/chạy được (bug wiring này có từ trước,
  không liên quan `Cache`, nhưng tiện sửa luôn vì đang đụng đúng file).
- Đã verify: `cargo check --benches` compile sạch cả hai bench, `cargo test --workspace` 45/45 pass (+1 integration
  test), `cargo fmt` áp dụng toàn repo theo đúng `rustfmt.toml`.

**Gemini lưu ý khi làm task 2**: không còn `src/storage/cache.rs`/`storage::cache::Cache` trong repo nữa — đừng tìm
hay tham chiếu tới nó. Cache duy nhất còn tồn tại và cần sửa cho task 2 vẫn là `ShardedCuckooTable`/`CuckooTable`
trong `src/engine/`.