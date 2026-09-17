# Plan "duck" — Thi hành ADR-0015

Duyệt ngày 2026-09-17. QĐ-1..QĐ-4 chốt lúc lập kế hoạch, QĐ-5 chốt trong lúc thi hành M2. QĐ-3 và QĐ-4 sửa đổi ADR-0015 và được ghi lại thành ADR-0016.

## Bối cảnh

ADR-0015 đã `accepted` ngày 2026-09-16 và định nghĩa lại bài toán Kallisto giải: không còn là "máy chủ secret hiệu năng cao", mà là **local read-only secrets resolver nói tiếng Vault KV-v2**, đằng sau là một file mã hoá trên bucket S3-compatible.

Code hiện tại chưa đi theo hướng đó. Đo đạc thực tế:

- ~6.769 dòng Rust. Đường đọc đi qua `KvEngine` (ShardedCuckooTable + TlsBTreeManager + RocksDbBackend + AsyncFlusher).
- `http_handler.rs` đang phục vụ **full CRUD**: `PUT`/`POST`/`DELETE`/`PATCH`, `delete`/`undelete`/`destroy`. ADR D1 nói những thứ này phải trả 403.
- Bốn thứ ADR bảo bỏ vẫn còn trong `Cargo.toml`: `rocksdb`, `rkyv`, `core_affinity`, `tikv-jemallocator`.
- Bốn thứ ADR bảo làm thì **chưa có dòng nào**: `components/kallisto_crypto/src/{master_key,rotation,shamir}.rs` rỗng, `components/kallisto_policy/src/{rbac,lease_mgr}.rs` rỗng, `components/kallisto_telemetry/src/{metrics,audit_log}.rs` rỗng. Không có code S3, không có parse config (ADR-0003 `accepted` từ 2026-08-17 nhưng chưa thi hành — binary mới chỉ đọc cờ dòng lệnh).
- `src/server/sys_handler.rs` trả health/seal-status **hardcode**, `"version": "1.13.0"`, `sealed: false` bất kể trạng thái thật.

Ngân sách ADR đặt ra: runtime ~1.500–2.000 dòng, phần test ít nhất tương đương. Nghĩa là công việc này **xoá nhiều hơn viết**, nhưng phần viết nằm gần như hoàn toàn ở chỗ mới.

## Năm quyết định chốt trong lúc lập kế hoạch

QĐ-1, QĐ-2 và QĐ-5 làm rõ ADR. **QĐ-3 và QĐ-4 sửa đổi ADR** — chúng đảo lại D9.1 và một nửa D10, nên phải được ghi lại ở chỗ người đọc ADR-0015 nhìn thấy (xem M0).

### QĐ-1: AES-256-GCM qua `aws-lc-rs`, không phải ChaCha20-Poly1305 thuần Rust

D13 viết "ví dụ ChaCha20-Poly1305". Chọn AES-256-GCM thay thế, vì:

1. `deny.toml` (kế thừa từ tikv/tikv) cấm **implementation**, không cấm thuật toán — `aes-gcm` của RustCrypto nằm ngay cạnh `chacha20poly1305` trong danh sách. Đổi thuật toán không né được việc phải sửa file.
2. Kallisto phải gọi HTTPS tới R2 → cần TLS → `rustls` 0.23 mặc định chạy trên `aws-lc-rs`. **aws-lc-rs đã có mặt trong cây phụ thuộc bất kể ta chọn gì.** Dùng luôn `aws_lc_rs::aead::AES_256_GCM` + `aws_lc_rs::hmac` thì thêm **không** dependency nào, và xoá được nhu cầu với `aes-gcm`, `hmac`, `sha2`.
3. AES-256-GCM đúng là thuật toán barrier của Vault thật — bớt một thứ trông lạ mắt với người đi audit.
4. aws-lc-rs bọc module AWS-LC đã được validate FIPS 140-3. Ta không nhắm compliance (ADR nói thẳng: chạm ngưỡng đó thì dựng OpenBao), nhưng cửa đó để ngỏ miễn phí thì không có lý do đóng lại.
5. AES-NI có trên mọi CPU server, và QĐ-3 đưa phép giải mã lên đường nóng, nên phần cứng đỡ được là điều đáng giá.

Giá phải trả: aws-lc-rs biên dịch C/asm nên cần `cmake` + `clang` lúc build. Vẫn nhẹ hơn RocksDB rất nhiều và đã được trả sẵn bởi yêu cầu TLS.

`deny.toml` nới đúng hai dòng: `{ name = "rustls" }` và `{ name = "ring" }`, kèm comment ghi rõ lý do và rằng phần còn lại của danh sách TiKV vẫn giữ nguyên. Kiểm chứng ngày 2026-09-17, sau khi M1 và M2 đã dựng xong: `cargo deny check` cho `bans ok` — rustls, reqwest và aws-lc-rs không chạm vào một lệnh cấm nào. Chỉ phải thêm `CDLA-Permissive-2.0` vào allow-list license, vì `webpki-roots` là bundle CA của Mozilla chứ không phải code.

Nonce 96-bit sinh ngẫu nhiên. Với tần suất seal của ta (vài nghìn lần trong cả đời dự án) xác suất trùng nonce là không đáng kể, và mỗi lần xoay data key là một khoá mới.

### QĐ-2: Lịch sử thuộc về git và bucket; Kallisto chỉ giữ một bộ đếm độ tươi

Kallisto **không** giữ lịch sử version của từng secret. File chỉ chứa giá trị hiện tại. Ai muốn xem giá trị cũ thì đọc git history hoặc bucket versioning.

Ngoại lệ bắt buộc, không uỷ quyền ra ngoài được: **số thứ tự file (`content_version`) nằm trong file, và Kallisto từ chối mọi file có số nhỏ hơn số nó đang giữ.** Kẻ tấn công trong mô hình D13 chính là người có quyền ghi bucket, nên bucket versioning nằm dưới tay nó; git thì ở bên kia CI. Chỉ có phép kiểm tra trong tiến trình mới chặn được. Số này được lưu kèm bản dự phòng trên đĩa để lúc khởi động lại cũng không bị lừa.

Hệ quả lên API, phải ghi vào bảng tương thích D7 của ADR-0015:

| Hành vi | Kallisto trả |
| --- | --- |
| `GET /v1/secret/data/<path>` | `metadata.version` = `content_version` của file |
| `?version=N` với N = `content_version` | như trên |
| `?version=N` với N khác | 404, đúng thân lỗi Vault |
| `GET /v1/secret/metadata/<path>` | đúng một mục trong `versions` |

### QĐ-3: Giữ thread-per-core và CPU pinning — sửa đổi D9.1

D9.1 nói bỏ pinning, chạy một luồng, vì "một app thì một luồng là quá đủ". **Tiền đề đó sai với một lớp app có thật:** có pattern lấy secret ngay trước mỗi lần dùng rồi huỷ đi luôn, không cache gì cả. Với pattern đó đường đọc *là* đường nóng, và rps không hề nhỏ.

Nên giữ nguyên kiến trúc kiểu Envoy đã chạy được: `src/event/worker.rs` + `src/server/listener.rs` — một Tokio runtime `current_thread` cho mỗi worker, ghim nhân bằng `core_affinity`, nhiều worker cùng nghe một cổng qua `SO_REUSEPORT`, kernel chia tải. Không work-stealing, không cache traffic chéo nhân. `core_affinity` và `socket2` ở lại trong `Cargo.toml`.

Số worker mặc định vẫn là 2 và vẫn cấu hình được, nên người chạy sidecar trong pod bị giới hạn CPU cứ đặt `workers: 1`. Cái bị bỏ là **giả định mặc định**, không phải khả năng.

Hai nửa còn lại của D9 vẫn đứng nguyên: memory footprint nhỏ (cuckoo table và arena bị xoá nên tự đạt), và native sidecar `initContainers` + `restartPolicy: Always`.

Ba ripple của quyết định này, đã ngấm vào các mốc bên dưới:

- **M5 đổi thiết kế.** Barrier trong RAM giờ nằm trên đường nóng: mỗi request là một phép AES-GCM open cộng một lần cấp phát bộ đệm. Phải dùng bộ đệm tái sử dụng theo thread (`thread_local`), zeroize sau mỗi response, thay vì cấp phát mới mỗi lần.
- **`tikv-jemallocator` ở lại**, ít nhất cho tới khi có số đo. Đường nóng có cấp phát, nên bỏ allocator phải là kết luận của benchmark chứ không phải của gu thẩm mỹ.
- **Vòng refresh phải chạy trên runtime riêng**, không nằm trên worker nào đang phục vụ, để một lần gọi bucket chậm không bao giờ chiếm mất một nhân đang trả secret.

### QĐ-4: Hexagonal bất đối xứng — hàn chết cổng ra, giữ port cổng nguồn — sửa đổi D10

D10 bỏ hexagonal toàn phần và nói "hai hàm là đủ — không cần port, không cần adapter". Sửa lại thành hai chiều khác nhau:

**Cổng ra: hàn chết.** `trait SecretEngine` và `EngineRegistry` bị xoá. Handler HTTP đọc thẳng `ArcSwap<Snapshot>` trong `AppState`. Không dispatch động, không `Arc<dyn>`, không `async_trait` trên đường nóng — mỗi lớp trừu tượng ở đây là một lần gọi gián tiếp trả bằng thông lượng, đổi lấy một khả năng thay thế mà không ai yêu cầu.

**Cổng nguồn: giữ port.** `trait SecretSource` với hai impl là bucket S3 và đĩa. Đây đúng là chỗ D10 tự nhận là "chỗ duy nhất đáng tách riêng"; khác biệt là ta làm nó thành một port thật thay vì hai hàm rời, vì nguồn thứ ba là chuyện có thể xảy ra còn cổng ra thì không. Nó chạy mỗi 30 giây nên chi phí dispatch động bằng không.

Tinh thần: *giữ lại kiến trúc đã chạy được, hàn chính thức vào lõi, bỏ trừu tượng hoá, xem thế nào đã.*

**Điều kiện xét lại:** nếu việc hàn cổng ra làm một thay đổi sau này trở nên khó, ta đã biết đường may nằm ở đâu — nó là ranh giới giữa `vault_api.rs` và `Snapshot`.

### QĐ-5: Tự ký SigV4 thay vì dùng `rusty-s3`

Phát hiện trong lúc thi hành M2: `rusty-s3` kéo theo `hmac`, `sha2`, `digest` và `md-5` của RustCrypto để ký SigV4 — **cả bốn đều nằm trong danh sách cấm của `deny.toml`**, và cả bốn vi phạm đều truy về đúng một crate đó. Nó cũng là crate duy nhất buộc phải thêm license BSD-2-Clause.

Chọn **tự ký**: `src/resolver/sigv4.rs` hiện thực SigV4 presigned URL cho đúng một lệnh `GET`, bằng `aws_lc_rs::hmac` và `aws_lc_rs::digest::SHA256` đã có sẵn trong cây. Khoảng 150 dòng, kiểm bằng bộ test vector AWS công bố (khoá ký `c4afb1cc...` cho us-east-1/iam ngày 2015-08-30).

Lý do phạm vi đủ nhỏ để tự nuôi: không POST, không multipart, không chunked upload, không object lock. Dùng xác thực bằng query string nên chỉ phải ký mỗi header `host`, và `GET` không có thân nên payload hash là hằng `UNSIGNED-PAYLOAD`.

Đổi lại: `deny.toml` giữ nguyên bốn lệnh cấm, bớt một dependency khỏi một công cụ bán bằng tính audit được, và bớt một ngoại lệ license. Ký sai thì bucket trả 403 ngay lập tức — hỏng to tiếng, không hỏng âm thầm.

## Hình dạng đích

```
components/kallisto_crypto/     (crate core_crypto)
  sealed_file.rs    định dạng file, seal/open, chống rollback     ~250
  barrier.rs        nửa bảo vệ RAM, bộ đệm theo thread            ~220
  hardening.rs      mlock, RLIMIT_CORE=0, PR_SET_DUMPABLE=0        ~60
components/kallisto_policy/     (crate policy_engine)
  matcher.rs        path syntax của Vault, read/list/deny         ~180
  token.rs          HMAC token → policy, so sánh constant-time     ~70
components/kallisto_telemetry/
  access_log.rs     drop-on-full, seq+timestamp lúc sinh, HMAC    ~150
  metrics.rs        Prometheus, gồm cả bộ đếm dòng bị drop         ~80
src/resolver/
  source.rs         trait SecretSource + impl bucket + impl đĩa   ~180
  snapshot.rs       Snapshot + ArcSwapOption                       ~80
  refresh.rs        vòng TTL trên runtime riêng, backoff          ~130
src/server/
  vault_api.rs      router chỉ-đọc, đọc thẳng Snapshot            ~450
  sys.rs            health/seal-status có version thật            ~120
  responses.rs      dựng thân JSON kiểu Vault, không qua serde    ~180
  listener.rs       GIỮ NGUYÊN — SO_REUSEPORT
src/event/worker.rs GIỮ NGUYÊN — thread-per-core, pinning
src/config.rs       YAML k8s-shaped, CLI > env > file > defaults  ~200
cmd/kallisto-ctl/   seal, verify, bump-version, validate          ~300
```

Tổng runtime ~2.400 dòng, vượt trần 2.000 của ADR. Trần đó chưa tính config, hardening, và chưa tính việc giữ lại lớp worker. Ghi nhận là vượt, không giả vờ vừa.

## Tám mốc

Chiến lược: **dựng song song rồi xoá**. `make test` phải xanh sau mỗi mốc. Engine cũ chỉ bị xoá ở M-X, sau khi M3 đã chạy được.

### M0 — Ghi lại hai sửa đổi, nới `deny.toml`

QĐ-3 và QĐ-4 ghi đè D9.1 và D10 của một ADR đang `accepted`. Theo đúng cách dự án đã xử lý ở lần trước: viết **ADR-0016**, và thêm vào ADR-0015 trường `amended-by: ADR-0016` cùng một khối `> [!NOTE]` trỏ sang. ADR-0016 ngắn — nó chỉ cần nói ba điều: pattern app lấy-rồi-huỷ làm tiền đề của D9.1 sai; cổng ra hàn chết còn cổng nguồn giữ port; và điều kiện xét lại của cả hai.

Nới `deny.toml` theo QĐ-1. Đây là blocker của M1, làm ngay chứ không làm kèm.

### M1 — Định dạng file mã hoá và lõi crypto

Bố cục file, header là plaintext nhưng được dùng làm AAD nên vẫn được xác thực:

```
magic "KALLISTO" (8B) | format_version u8 | content_version u64 LE | nonce (12B) | ciphertext+tag
```

Đọc được `content_version` **trước khi** giải mã, nên từ chối bản cũ mà không tốn một phép crypto nào; sửa số đó thì tag fail vì nó nằm trong AAD. Thân đã giải mã là JSON `serde`: `{version, secrets, policies, tokens}`, và `version` bên trong phải khớp header — thắt hai lần.

API: `seal(plain, &Key, version)`, `open(bytes, &Key, held_version)`.
`SealError`: `BadMagic`, `UnsupportedFormat`, `Rollback{held, offered}`, `AuthFailed`, `VersionMismatch`. Không biến thể nào được mang giá trị secret hay đường dẫn — `tests/security_invariants.rs` đã có khuôn kiểm tra đúng chuyện này cho `EngineError`, mở rộng nó sang `SealError`.

Test: lật từng byte → `AuthFailed`; sai khoá → `AuthFailed`; đang giữ v7 mà đưa v5 → `Rollback`; header khai v9 mà thân ghi v5 → `AuthFailed`.

Kiểm cross-compile musl ngay ở mốc này, không để tới lúc dựng Docker — aws-lc-rs là dependency C đầu tiên của bản mới.

### M2 — Vòng resolver

`source.rs` — port theo QĐ-4:

```rust
trait SecretSource {
    async fn fetch(&self, etag: Option<&str>) -> Fetched;
}
enum Fetched { NotModified, Body { bytes: Vec<u8>, etag: Option<String> }, Unavailable(SourceError) }
```

`BucketSource` dùng conditional GET với `If-None-Match` (D2 bắt buộc), ký request bằng `sigv4.rs` của chính dự án (**QĐ-5**), gửi bằng `reqwest` cấu hình `default-features = false, features = ["rustls-tls"]` — tránh AWS SDK chính thức đúng như D2 dặn. `DiskSource` đọc file, dùng cho test và cho bản dự phòng.

`snapshot.rs` — `ArcSwapOption<Snapshot>`. `None` nghĩa là chưa sẵn sàng → mọi request trả 503, tương đương Vault *sealed* (D14). Không bao giờ thoát tiến trình vì bucket chết.

`refresh.rs` — **runtime riêng, một thread, không ghim nhân** (QĐ-3). `tokio::time::interval` mặc định 30s, `tokio::sync::Notify` cho force-refresh. Gặp `SlowDown`/429 từ bucket thì backoff và **vẫn phục vụ bảng đang giữ** (D14). File hỏng hoặc rollback thì **giữ bảng cũ** và ghi error log.

Bản dự phòng trên đĩa (D5): mỗi lần fetch thành công thì ghi **nguyên bytes đã mã hoá** ra `cache_dir` bằng write-rename nguyên tử, kèm sidecar `.version`. Lúc boot mà bucket chưa lên thì đọc từ đó. Không bao giờ ghi bản giải mã.

### M3 — HTTP nói tiếng Vault, chỉ đọc  ← **lằn ranh MVP**

Router mới ở `src/server/vault_api.rs`. `AppState` giờ mang `Arc<ArcSwapOption<Snapshot>>` thay cho `Arc<EngineRegistry>` — đây chính là chỗ cổng ra bị hàn (QĐ-4). Giữ lại `extract_mount_and_path` từ `http_handler.rs` (đã có test, dùng lại nguyên).

`responses.rs` giữ nguyên cách dựng thân JSON bằng `String::push_str` như `read_secret` hiện tại. Đường đọc **không parse JSON và không đi qua serde** — nó chỉ ghép chuỗi quanh giá trị đã có sẵn trong Snapshot.

Hỗ trợ:
- `GET /v1/:mount/data/*path`
- `GET|LIST /v1/:mount/metadata/*path` — **cả hai method**. Vault chấp nhận method `LIST` lẫn `GET ?list=true`, và các SDK chia nhau dùng cả hai. axum không biết `LIST`, nên phải qua `method_routing::any` rồi tự phân nhánh theo `Method::from_bytes(b"LIST")`. Đây là cái bẫy dễ trượt bài test con vịt nhất.
- `GET /v1/sys/health` — `sealed` phản ánh trạng thái thật; thêm `kallisto_file_version`, `kallisto_etag`, `kallisto_loaded_at` (D4). Khi sealed thì trả 503 đúng như Vault.
- `GET /v1/sys/seal-status`
- `GET /v1/auth/token/lookup-self`, `POST /v1/auth/token/renew-self` (D7 nói rõ đây không phải phần thừa)

Từ chối, 403 kèm `{"errors":["permission denied"]}`:
`PUT`/`POST`/`PATCH`/`DELETE` trên `data`, và toàn bộ `delete`/`undelete`/`destroy`/`subkeys`.

429 kèm `Retry-After` từ một token bucket đơn giản; 503 khi chưa nạp được file (D14).

Bind **mặc định `127.0.0.1`**, và từ chối khởi động nếu config trỏ ra địa chỉ khác mà không có cờ `--i-accept-the-risk` — ràng buộc vận hành số 1 của ADR là "không bao giờ mở cổng 8200 ra ngoài localhost", nên nó phải được cưỡng chế bằng code chứ không bằng tài liệu.

`src/config.rs` thi hành ADR-0003: YAML kiểu Kubernetes, `#[serde(deny_unknown_fields)]`, thứ tự CLI > env > file > defaults. Thêm `workers`, endpoint bucket, TTL reload, `cache_dir`.

**Tới đây Kallisto đã chạy được thật**: app nói Vault SDK vào localhost, đọc secret từ file mã hoá trên R2/Garage, bucket chết vẫn sống. Dừng ở đây vẫn có sản phẩm. Chạy `make bench-laptop` ở mốc này để có số nền trước khi M5 đặt crypto lên đường nóng.

### M4 — Phân quyền

`X-Vault-Token` → HMAC-SHA256 có khoá → tra bảng `tokens` trong file → tên policy → so khớp đường dẫn.

Giữ nguyên tật của KV-v2 (D8): quyền đọc là `secret/data/payment/*`, quyền liệt kê là `secret/metadata/payment/*`. Ba capability: `read`, `list`, `deny`; `deny` thắng tất cả. Wildcard `*` cuối chuỗi và `+` một segment, đúng ngữ nghĩa Vault.

So sánh HMAC bằng hàm constant-time. Việc này **mở khoá E2 của ADR-0013**, hiện đang ghi là blocked trong `verification-status.md` vì tính năng chưa tồn tại.

Không có token, hoặc token lạ → 403 `permission denied` (Vault trả 403 chứ không phải 401).

Bảng token dựng sẵn thành `HashMap` lúc tráo Snapshot, để đường nóng chỉ còn một phép HMAC và một lần tra bảng.

### M5 — Nửa bảo vệ RAM

Khoá ngẫu nhiên sinh lúc boot; mở file **một lần** ở vòng refresh, mã hoá lại **từng secret** bằng khoá đó, xoá sạch bản chữ trần của cả file. Giải mã đúng secret được hỏi ngay lúc phục vụ.

Theo QĐ-3, đây là đường nóng, nên: **bộ đệm giải mã là `thread_local` tái sử dụng**, zeroize sau khi response đi ra, không cấp phát mới mỗi request. Mỗi worker một bộ đệm, không chia sẻ, không khoá.

`hardening.rs`: `setrlimit(RLIMIT_CORE, 0)`, `prctl(PR_SET_DUMPABLE, 0)`, `mlock` trang chứa khoá.

Test kiểm được: quét toàn bộ byte của `Snapshot` đang sống, khẳng định không chứa giá trị secret; khẳng định bộ đệm được zeroize sau response. Không thử chứng minh điều ADR đã nói thẳng là không chống được (root đọc RAM).

Chạy lại `make bench-laptop` và so với số nền của M3. Mức tụt là con số phải báo cáo, không phải con số để giấu.

### M6 — Access log và error log

**Xoá `components/kallisto_telemetry/src/audit_log.rs`.** D15 cấm dùng chữ "audit" ở tên biến, tên file, config key và tài liệu; file rỗng mang cái tên đó là cái bẫy đầu tiên phải dọn.

`access_log.rs` dùng `kallisto_queue` (hàng đợi Vyukov, tái dụng từ ADR-0008). Theo QĐ-3 thì giờ có **nhiều producer thật** — mỗi worker một, nên phần MPMC của hàng đợi được dùng đúng nghĩa lần này, khác với ghi chú "một người ghi là đủ" trong D15. Đúng theo ràng buộc D15, mỗi dòng mang **sequence number và timestamp gán ngay lúc sinh ra**, vì nhiều producer thì thứ tự tới consumer không còn là thứ tự sinh.

Hàng đợi đầy thì **drop, không bao giờ chặn đường đọc**, và đếm số dòng mất thành `kallisto_access_log_dropped_total`.

Vệ sinh: không bao giờ ghi giá trị secret; token và đường dẫn đi qua **HMAC có khoá**, áp cho **cả** access log lẫn error log.

### M7 — CLI

`cmd/kallisto-ctl` đang là stub `ratatui`. Bỏ TUI (ADR-0004 vẫn `suspended`), đổi binary thành `kallisto-ctl`:

- `seal --in plain.json --out secrets.kal --version N`
- `verify --in secrets.kal` — kiểm tag và số version, không in ra nội dung
- `bump-version --in secrets.kal`
- `validate --config kallisto.yaml` — lời hứa trong phần Confirmation của ADR-0003
- `open` có, nhưng đòi `--yes-print-secrets-to-stdout`

Khoá đọc từ env, không nhận qua tham số dòng lệnh (tham số lộ qua `ps`).

### M8 — Bộ test con vịt

Đây là fitness function của cả dự án, không phải phần phụ.

`tests/duck/docker-compose.yml`: Kallisto + Garage (hoặc MinIO) + ba container client chạy **SDK Vault thật** của Go, PHP, Python.

- Ca thuận: mọi dòng ✅ trong bảng D7 phải chạy qua mà SDK không nhận ra khác biệt.
- Ca nghịch: mọi dòng ❌ phải nhận **đúng mã HTTP và đúng thân JSON** Vault trả.
- Ca xấu tường minh: file giả mạo, file cũ bị đặt lại, bucket chết, sai khoá, token đã thu hồi, và **hàng đợi log đầy phải drop mà vẫn phục vụ**.
- Một ca riêng cho pattern đã sinh ra QĐ-3: client lấy secret rồi huỷ ngay, lặp ở rps cao, chạy qua nhiều worker.

Thay thế `tests/e2e_vault_compat.rs` hiện tại — nó test `kv put`/`patch`/`delete`/`undelete`/`destroy`, tức là toàn bộ thứ giờ phải trả 403.

### M-X — Commit xoá

Chạy sau khi M3 ổn định, thành **một commit riêng** để nó là hồ sơ ghi lại cái gì bị bỏ.

Xoá code:
- `src/engine/` trừ `error.rs`: `kv_engine`, `btree_index`, `tls_btree_manager`, `sharded_cuckoo_table`, `cuckoo_table/`, `lock_free_queue`, và — theo QĐ-4 — `traits.rs` cùng `engine_registry.rs`
- `src/storage/` toàn bộ (`rocksdb_backend`, `async_flusher`)
- `src/server/{http_handler,admin_handler,sys_handler}.rs`
- `components/kallisto_cluster` — gossip `foca` và admin server cổng 8202
- `components/kallisto_kv_model` — nó mô hình hoá **ngữ nghĩa ghi** (put/delete/undelete/destroy + CAS), không còn đối tượng phục vụ. Gỡ khỏi workspace members; git giữ lại.
- `tests/test_phase4.rs`, `tests/e2e_vault_compat.rs`
- `fuzz/fuzz_targets/rkyv_roundtrip.rs` và mục `[[bin]]` của nó

**Không** xoá: `src/event/worker.rs`, `src/server/listener.rs` (QĐ-3).

Xoá dependency: `rocksdb`, `rkyv`, `dashmap`, `crossbeam-channel`, `crossbeam-epoch`, `siphasher`, `async-trait`, `foca`, `ratatui`, và `sonic-rs`. Bỏ `sonic-rs` an toàn vì SIMD của nó sinh ra cho đường ghi: đường đọc mới không parse JSON, còn chỗ duy nhất còn parse là vòng refresh chạy hai lần một phút — `serde_json` thừa sức, và bỏ nó là bỏ luôn một khối unsafe SIMD.

**Giữ lại**: `core_affinity`, `socket2`, `tikv-jemallocator` (QĐ-3).

Sửa hạ tầng:
- `Makefile`: xoá `verify-miri-rkyv`, sửa `verify-miri` còn `verify-miri-queue`, xoá dòng `fuzz run rkyv_roundtrip`, xoá `durability` (không còn gì để ghi bền), thêm `duck`
- `deny.toml`: bỏ mục `ignore` cho advisory của rkyv 0.7 — D12 nói đúng, nó biến từ bài toán data migration thành một dòng bị xoá
- `.github/workflows/rust-ci.yml`, `main-publish.yml`, `alpha-publish.yml`: bỏ OS dependency cho `librocksdb-sys`, thêm `cmake`/`clang` cho aws-lc-rs
- `Dockerfile`: đích là static musl + distroless

Sửa tài liệu:
- `AGENTS.md` mục "Code Organization" và "Architecture" đang mô tả kiến trúc cũ tới từng file — viết lại, giữ nguyên phần thread-per-core vì phần đó vẫn đúng
- `docs/.../roadmap.md`, `verification-status.md` (E2 chuyển từ blocked sang verified ở M4)
- `README.md` — tagline mới, bảng tương thích D7, và mục "không chống được"
- ADR-0015: bổ sung dòng `?version=N` vào bảng D7 theo QĐ-2

## Kiểm chứng

Sau mỗi mốc:

```bash
make dev        # format + clippy + deny + test  ← deny phải xanh, đó là điểm QĐ-1 được kiểm
make test
```

Sau M3, kiểm bằng tay với Vault CLI thật:

```bash
kallisto-ctl seal --in /tmp/plain.json --out /tmp/secrets.kal --version 1
# đẩy lên MinIO/Garage local
make run-server
VAULT_ADDR=http://127.0.0.1:8200 VAULT_TOKEN=... vault kv get secret/app/db
VAULT_ADDR=http://127.0.0.1:8200 vault kv put secret/app/db x=y   # phải là 403 permission denied
curl -s localhost:8200/v1/sys/health | jq .kallisto_file_version
```

Cổng thông lượng, hệ quả trực tiếp của QĐ-3 — đo bằng `make bench-duck` tại ba điểm:

| Điểm đo | Kỳ vọng | Đã đo |
| --- | --- | --- |
| M3 (chưa có barrier) | ít nhất bằng số nền của engine cũ; kiến trúc mới ít lớp hơn nên không có lý do tụt | ✅ xem dưới |
| M5 (có barrier trên đường nóng) | mức tụt phải đo được và phải báo cáo | ⬜ |
| M8 | chạy ca lấy-rồi-huỷ ở rps cao qua nhiều worker | ⬜ |

**Số nền M3**, laptop 8 nhân, 4 worker, 100 connection, wrk2 chạy cùng máy:

| | 30k req/s (đúng tham số `bench-laptop` cũ) | bão hoà, mục tiêu 200k |
| --- | --- | --- |
| Thông lượng | 28.3k req/s | **107.0k req/s** |
| Trung bình | 1.38 ms | 2.21 s |
| p50 | 1.29 ms | 2.22 s |
| p99 | 3.51 ms | 4.51 s |

Cột trái so trực tiếp được với engine cũ: `make help` từ trước vẫn ghi kỳ vọng của `bench-laptop` là *"~1.5ms avg"* ở đúng 30k req/s, và đường đọc mới đo được 1.38 ms. Không tụt. Cột phải là trần thật của máy này — wrk2 ngồi chung 8 nhân với 4 worker, nên nó là sàn của trần chứ không phải trần.

Lưu ý về công cụ đo: `run_release_bench.sh` cũ gieo dữ liệu bằng `PUT` qua HTTP, mà mọi `PUT` bây giờ trả 403 theo đúng D1. Nó được thay bằng `benchmarks/server/run_duck_bench.sh`, gieo bằng một file đã seal. Script mới **nâng token bucket lên rất cao trong file config của nó** — nếu để mặc định 20k/s mỗi worker thì thứ được đo là cái bucket, không phải đường đọc.

Sau M8, `make duck` là cổng cuối: ba SDK thật, ca thuận và ca nghịch, ca xấu tường minh.

Ba dấu hiệu red line của ADR phải được kiểm tự động, không kiểm bằng niềm tin:
1. cổng 8200 chỉ nghe localhost → test khẳng định bind mặc định và khẳng định cấu hình sai bị từ chối lúc khởi động
2. phân quyền có thật → M4 kèm test token bị thu hồi
3. bộ test tương thích tồn tại → chính M8

## Rủi ro đã biết

- **`deny.toml` là blocker của M1.** Không nới được thì M1 không bắt đầu được. Đó là lý do nó nằm ở M0.
- **Method `LIST`** — axum không có sẵn. Nếu làm hụt thì `vault kv list` trượt và bài test con vịt trượt theo, nhưng mọi test đọc vẫn xanh nên rất dễ tưởng là xong.
- **Barrier trong RAM giờ nằm trên đường nóng.** Đây là cái giá của QĐ-3 và nó có thật: một phép AES-GCM open cho mỗi request. Nếu số đo ở M5 tụt quá mức chấp nhận được thì lựa chọn là cho phép tắt barrier qua config với cảnh báo rõ ràng, chứ không phải im lặng bỏ nó.
- **`?version=N` là một cắt giảm nhìn thấy được từ phía app.** Phải nằm trong bảng D7 và trong README trước khi có người dùng thật, đúng nguyên tắc "giới hạn giấu đi là một cái bẫy".
- **aws-lc-rs cần `cmake` + `clang` lúc build.** Nhẹ hơn RocksDB nhiều nhưng không phải bằng không; cross-compile musl phải kiểm ở M1.
- **Ngân sách vượt ~400 dòng** so với trần 2.000 của ADR, do config, hardening và lớp worker được giữ lại chưa được tính vào trần đó.
