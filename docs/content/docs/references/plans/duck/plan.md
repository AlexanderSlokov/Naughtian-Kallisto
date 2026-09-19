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

## Sáu quyết định

QĐ-1..QĐ-4 chốt lúc lập kế hoạch; QĐ-5 chốt trong lúc thi hành M2; QĐ-6 chốt trong lúc thi hành M4.

QĐ-1, QĐ-2, QĐ-5 và QĐ-6 làm rõ ADR. **QĐ-3 và QĐ-4 sửa đổi ADR** — chúng đảo lại D9.1 và một nửa D10, nên phải được ghi lại ở chỗ người đọc ADR-0015 nhìn thấy (xem M0).

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

### QĐ-6: Khoá băm token nằm **trong** file, không dẫn xuất từ seal key

D8 nói file chỉ lưu `hash(token)`, nhưng không nói khoá của phép băm đó ở đâu. Hai lựa chọn, và lựa chọn hiển nhiên là lựa chọn sai:

* **Dẫn xuất từ seal key.** Không thêm gì vào định dạng. Nhưng xoay seal key sẽ **giết sạch mọi token trong fleet**: operator cầm trong tay các *hash*, không cầm token, nên không có gì để tính lại. Một thao tác vận hành bình thường trở thành sự cố toàn hệ thống.
* **Nằm trong file** (`token_key`, hex, `#[serde(default)]`). Xoay seal key chỉ là seal lại đúng bảng đó; mọi token tiếp tục chạy. Khoá được bảo vệ bởi chính lớp mã hoá của file, và được zeroize khi drop.

Chọn cái thứ hai. Kèm một ràng buộc toàn vẹn: file có `tokens` mà **không** có `token_key` thì không token nào có thể xác thực được — đó là file hỏng, và resolver **từ chối** nó thay vì phục vụ với phân quyền tắt âm thầm. Không có cả hai là trường hợp sidecar của D8, hợp lệ, và `sys/health` báo `kallisto_authorization: "none"` để nó không phải là thứ phải tự phát hiện.

Băm là HMAC-SHA256 có nhãn phân tách miền (`kallisto/token/v1\0`), không phải digest trần: bản plaintext của file đi qua editor, git và một job CI trước khi được seal, nên hash trần của một token ngắn chỉ cách từ điển một bước.

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

> **Việc này đã bị hoãn, và hoá đơn tới đúng chỗ câu trên cảnh báo.** Target musl không được cài ở M1, Dockerfile viết ở M-X mà không build lần nào, và nó hỏng **hai tầng** khi CI chạy thật. Chi tiết ở mục *Sau M-X — sửa pipeline*. Bài học không phải "musl khó" mà là: một bước kiểm mà kế hoạch tự đặt ra rồi tự bỏ qua thì nó không biến mất, nó chỉ trôi tới chỗ đắt hơn.

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

Bảng token dựng sẵn lúc tráo Snapshot, để đường nóng chỉ còn một phép HMAC và một lần tra bảng.

**Đã thi hành, có hai chỗ lệch khỏi kế hoạch — cả hai đã ghi vào bảng D7 của ADR-0015:**

* Bảng token **không** là `HashMap` mà là một `Vec` quét tuyến tính bằng `constant_time::verify_slices_are_equal`. `HashMap::get` thoát sớm và so sánh chuỗi theo kiểu ngắt giữa chừng, tức là phép so sánh mà E2 nói tới sẽ không tồn tại — nó chỉ còn là một lời chú thích. Với vài chục token (đúng cỡ D11 đặt ra) phép quét rẻ hơn chính phép HMAC đứng trước nó; vài nghìn token thì cần hình dạng khác, và cũng có nghĩa Kallisto đang bị dùng sai.
* **`deny` thắng tuyệt đối** thay vì thua một đường dẫn cụ thể hơn như Vault thật. Khác biệt chỉ xuất hiện với policy tự mâu thuẫn, và Kallisto lệch về phía **từ chối**.

E2 và E3 của ADR-0013 chuyển từ *blocked* sang **proven**, và mỗi test đều đã được kiểm bằng cách phá hỏng implementation cho nó fail. Một chi tiết đáng ghi: bản E2 đầu tiên — ba token ở ba vị trí, khẳng định mỗi cái resolve đúng — **sống sót** một implementation chỉ so 4 byte đầu của hash. Bản hiện tại dựng một bảng chỉ gồm *near miss* (hash thật lật một bit ở năm vị trí) và khẳng định không cái nào khớp. Giới hạn còn lại — rằng vòng lặp duyệt hết mọi entry chứ không return ở lần khớp đầu — **không** kiểm được bằng test, và `verification-status.md` nói thẳng điều đó.

### M5 — Nửa bảo vệ RAM

Khoá ngẫu nhiên sinh lúc boot; mở file **một lần** ở vòng refresh, mã hoá lại **từng secret** bằng khoá đó, xoá sạch bản chữ trần của cả file. Giải mã đúng secret được hỏi ngay lúc phục vụ.

Theo QĐ-3, đây là đường nóng, nên: **bộ đệm giải mã là `thread_local` tái sử dụng**, zeroize sau khi response đi ra, không cấp phát mới mỗi request. Mỗi worker một bộ đệm, không chia sẻ, không khoá.

`hardening.rs`: `setrlimit(RLIMIT_CORE, 0)`, `prctl(PR_SET_DUMPABLE, 0)`, `mlock` trang chứa khoá.

Test kiểm được: quét toàn bộ byte của `Snapshot` đang sống, khẳng định không chứa giá trị secret; khẳng định bộ đệm được zeroize sau response. Không thử chứng minh điều ADR đã nói thẳng là không chống được (root đọc RAM).

Chạy lại `make bench-laptop` và so với số nền của M3. Mức tụt là con số phải báo cáo, không phải con số để giấu.

**Đã thi hành. Một chỗ sửa đi ngược lên M1, và nó là chỗ quan trọng nhất của mốc này:**

Kế hoạch nói "xoá sạch bản chữ trần của cả file". M1 đã làm đúng phần đó — bộ đệm giải mã là `Zeroizing<Vec<u8>>`. Nhưng ngay sau đó `serde_json::from_slice` **dựng một bản sao thứ hai của mọi secret** thành `String` và `Value` sở hữu, trên heap, và không ai xoá chúng khi drop. Bản sao đó sống sót trong vùng nhớ đã free — tức là đúng thứ chui vào core dump và vào swap, hai tai nạn mà nửa bảo vệ RAM sinh ra để chặn. Dựng barrier mà để nguyên bản sao đó thì barrier chỉ là đồ trang trí.

Nên `open()` không trả `Contents` sở hữu nữa. Nó trả `Opened` — plaintext còn nằm trong chính bộ đệm tự xoá — và `Opened::view()` cho mượn vào đó: `secrets: BTreeMap<&str, &RawValue>`, `token_key: Option<&str>`. Snapshot seal thẳng từ những lát mượn ấy. Không có bước render lại, không có bản sao sở hữu nào của chữ trần được tạo ra bao giờ. `policies` và `tokens` vẫn sở hữu — chúng là path pattern, tên policy và hash có khoá, không phải secret.

Phần còn lại đúng kế hoạch: khoá ngẫu nhiên **theo từng snapshot** (không phải theo tiến trình — file mới là khoá mới, và khoá cũ bị zeroize cùng snapshot cũ), nonce đếm tăng (khoá tươi nên bộ đếm là đủ và không cần gọi RNG mỗi secret), bộ đệm giải mã `thread_local` không chia sẻ không khoá, `mlock` trang chứa `LessSafeKey` với `munlock` lúc drop — thiếu cái `munlock` thì mỗi lần đổi file lại bỏ lại một trang bị khoá cho tới khi `RLIMIT_MEMLOCK` từ chối, và cái hỏng sẽ là khoá barrier âm thầm không còn được ghim nữa.

**Số đo.** Laptop 8 nhân, 4 worker, wrk2 chạy cùng máy.

Lần đo đầu cho ra kết quả vô lý: M5 **nhanh hơn** M3 ở điểm bão hoà (118–120k so với 107k). Thêm một phép AES-GCM mỗi request mà nhanh lên thì không phải kết quả, đó là lỗi phương pháp — M5 được đo lúc máy còn nguội, M3 đo sau đó khi máy đã nóng. Cách chữa là **chạy xen kẽ hai binary trong cùng một vòng**, và con số đổi hẳn:

| Bão hoà (mục tiêu 200k) | M3 | M5 |
| --- | --- | --- |
| vòng 1 | 98.8k | 100.6k |
| vòng 2 | 99.1k | 97.5k |
| vòng 3 | 98.9k | 98.6k |
| **trung bình** | **98.9k req/s** | **98.9k req/s** |

Bằng nhau. Ở 30k req/s, đo hai vòng xen kẽ, độ trễ có một khác biệt nhỏ nhưng **lặp lại được**:

| 30k req/s | M3 | M5 |
| --- | --- | --- |
| p50 | 1.27 ms | 1.28–1.29 ms |
| p99 | 3.49 ms | 3.69–3.71 ms |

Đuôi p99 tụt khoảng **0.2 ms (~6%)**; p50 gần như không đổi. Đó là mức tụt phải báo cáo.

Chi phí đo riêng bằng `cargo bench --bench barrier_bench`, cùng một response body dựng từ hai phía:

| | thời gian |
| --- | --- |
| Dựng body từ chữ trần (hình dạng M3) | ~400 ns |
| Mở rồi dựng body (hình dạng M5) | ~836 ns |
| Chỉ phép mở | ~349 ns |
| Seal cả file 64 secret (chạy 2 lần/phút) | ~36 µs |

**Barrier tốn khoảng 350–430 ns mỗi request.** Ở trần ~99k req/s trên 4 worker, mỗi worker có ~40 µs cho một request, nên đây là dưới 1% ngân sách — chìm dưới nhiễu ở thông lượng, và chỉ ló ra ở đuôi p99. Không cần đến phương án tắt barrier qua config mà phần rủi ro đã dự phòng.

**Quét bộ nhớ tiến trình thật** (`cargo run --example cleartext_scan`), đọc mọi vùng anonymous và heap của chính nó:

| | số bản chữ trần trong RAM |
| --- | --- |
| nền, trước khi mở file | baseline |
| sau khi dựng snapshot | **+0** |
| trong lúc một response body còn sống | +2 |
| sau khi body đó bị drop | +2 |

Dòng thứ hai là điều M5 phải chứng minh, và example `assert!` đúng vào nó. Hai dòng cuối là điều D13 đã nói thẳng: thân response mang chữ trần, và `free()` không phải `zeroize` — bản sao ấy còn nằm đó cho tới khi có thứ khác ghi đè.

Bản thân công cụ quét cũng có một bài học: bản đầu tiên cấp phát vài MB mỗi lần quét, và chính những lần đọc đó rơi trúng các block vừa được free mà nó sắp nhìn vào — nên nó báo "sạch" với mọi đầu vào. Bản hiện tại cấp phát hết mọi bộ đệm trước phép đo đầu tiên và sau đó không cấp phát nữa.

Và một câu tôi đã viết sai rồi phải rút lại, ghi ở đây vì nó là loại sai dễ lặp: phép quét bộ nhớ **từ ngoài vào** bị từ chối, và tôi quy cho `prctl(PR_SET_DUMPABLE, 0)`. Kiểm lại bằng hai tiến trình chỉ khác nhau đúng lời gọi đó thì **quyền sở hữu `/proc/<pid>` giống hệt nhau** — kernel hiện tại không đổi owner nữa. Thứ thật sự chặn là Yama `ptrace_scope = 1` của máy này: chỉ tiến trình cha mới được attach. Một `/proc/<pid>/mem` bị từ chối **không** phải bằng chứng rằng `PR_SET_DUMPABLE` đã có tác dụng, và đó cũng là lý do example phải quét từ *bên trong* tiến trình.

Cái kiểm được chắc chắn là `RLIMIT_CORE`: `/proc/<pid>/limits` của server đang chạy ghi `Max core file size 0`.

**Nói rõ cái barrier này không làm được**, ngoài điều ADR đã nói về root: **thân response mang secret dưới dạng chữ trần** từ lúc được dựng cho tới khi socket lấy đi. Đó là bản chất của việc phục vụ một secret, và D13 nói đúng câu đó — "tại mọi thời điểm chỉ có vài secret đang được gửi đi là ở dạng chữ trần".

### M6 — Access log và error log

**Xoá `components/kallisto_telemetry/src/audit_log.rs`.** D15 cấm dùng chữ "audit" ở tên biến, tên file, config key và tài liệu; file rỗng mang cái tên đó là cái bẫy đầu tiên phải dọn.

`access_log.rs` dùng `kallisto_queue` (hàng đợi Vyukov, tái dụng từ ADR-0008). Theo QĐ-3 thì giờ có **nhiều producer thật** — mỗi worker một, nên phần MPMC của hàng đợi được dùng đúng nghĩa lần này, khác với ghi chú "một người ghi là đủ" trong D15. Đúng theo ràng buộc D15, mỗi dòng mang **sequence number và timestamp gán ngay lúc sinh ra**, vì nhiều producer thì thứ tự tới consumer không còn là thứ tự sinh.

Hàng đợi đầy thì **drop, không bao giờ chặn đường đọc**, và đếm số dòng mất thành `kallisto_access_log_dropped_total`.

Vệ sinh: không bao giờ ghi giá trị secret; token và đường dẫn đi qua **HMAC có khoá**, áp cho **cả** access log lẫn error log.

**Đã thi hành.** Hai quyết định chốt trước khi viết, cả hai đều đổi hình dạng của mốc:

**QĐ-7 — khoá HMAC của log lấy từ `token_key` trong sealed file.** Định danh token trong log **chính là** khoá của hàng đó trong map `tokens`: operator mở file ra là đọc được dòng log thuộc entry nào, không cần kho khoá thứ hai, không đổi định dạng file, và ổn định qua restart.

Nhưng đường dẫn **không** được băm dưới cùng nhãn với token, và đây là chỗ suýt trượt. Đường dẫn là **do người gọi chọn**: ai gửi được `GET /v1/secret/data/<gì cũng được>` rồi đọc access log sẽ có trong tay một oracle sinh `HMAC(token_key, chuỗi tuỳ ý)` — đúng vật liệu để dựng bảng tra ngược đánh vào cột token của file. Nên `token_id` giữ nhãn cũ (khớp thẳng với file), còn `path_id` đi dưới `LogKey = HMAC(token_key, "kallisto/log/v1\0")`. Hai PRF khác nhau. File không có token table thì `LogKey` là khoá ngẫu nhiên theo tiến trình, và server nói rõ điều đó.

**QĐ-8 — số dòng bị drop phải scrape được.** Không giấu vào `sys/health`. Hàng đợi tràn nghĩa là daemon đang vứt bớt sổ sách để giữ cho secret vẫn được phục vụ; đó là quyết định đúng, và cũng là thứ operator phải bị đánh thức để xem cái gì đang ném tải vào một tiến trình chịu được hàng chục nghìn read mỗi giây. Một metric scrape được thì bắn được alert; một trường JSON thì không. Nên có `GET /v1/sys/metrics` (đúng đường của Vault), Prometheus text exposition, **tự viết** — crate `prometheus` dùng `RwLock<HashMap>` cho registry, tăng bộ đếm qua nó là đặt cache line dùng chung vào giữa đường đọc, đúng thứ QĐ-3 tồn tại để tránh; nó còn kéo `protobuf` cho một định dạng Prometheus đã bỏ. Bộ đếm là **một mảng, mỗi worker một ô**, dựng trước lúc spawn: worker chỉ ghi ô của mình, còn scrape (rơi vào worker nào là ngẫu nhiên vì `SO_REUSEPORT`) cộng cả mảng.

**Chỗ lệch kế hoạch: HMAC không được nằm trên đường nóng.** Một phép HMAC-SHA256 tốn cỡ đúng bằng cả cái barrier của M5. Trả nó mỗi request để ghi một dòng log là làm việc quan sát đắt hơn việc phục vụ. Không cần trả: tập đường dẫn **đã biết trước** — nó là khoá của map `secrets` — nên `path_id` được tính sẵn lúc dựng Snapshot, hai lần một phút, cất cạnh mỗi secret đã seal. Đường dẫn lạ (404, và mọi chuỗi kẻ tấn công bịa ra) mới băm tại chỗ, và nhánh đó đã có token bucket chặn trước. `token_id` thì tái dùng phép băm `TokenTable::lookup` vốn đã tính từ M4.

Kèm theo đó là một phát hiện ngược lên M4: **`TokenKey::hash` đang dựng lại `hmac::Key` ở mỗi lần gọi**, tức mỗi request từ M4 tới giờ đều trả tiền cho phép dẫn xuất ipad/opad trước khi băm. Giữ sẵn `hmac::Key` làm cả đường phân quyền lẫn đường log rẻ đi. Phần dư đã ghi nhận chứ không giấu: `aws_lc_rs::hmac::Key` giữ khối ipad/opad và không tự zeroize — cùng loại phần dư mà `LessSafeKey` để lại trong barrier.

**Access log là một layer, không phải lời gọi trong từng handler.** Giá trị của access log nằm ở chỗ **mọi** request đều có mặt; rải ra mười hai nhánh thì cái nhánh bị quên là cái vô hình — test vẫn xanh, log vẫn trông khoẻ mạnh, và những request biến mất đúng là những request đáng xem. Một chỗ duy nhất, và không route mới nào lách qua được. Giá phải trả là một lần `ArcSwap` load nữa và một future bị box mỗi request.

**Dòng log không cấp phát**: `[u8; 192]` nội tuyến, mọi trường có biên trên biết trước. Người ghi là một `std::thread` thường (không phải task tokio), dequeue theo lô, backoff tới ~1 ms khi rỗng — không Condvar, vì đánh thức bằng Condvar là đặt một mutex lên đường đọc.

**Ràng buộc "không bao giờ gọi là audit log" giờ là một gate.** `d15_nothing_in_the_code_is_named_audit` quét `src/`, `components/*/src/`, `cmd/*/src/`, `*.yaml`, `*.toml` và fail nếu chữ đó xuất hiện ngoài comment. Nó **bắt được lỗi ngay lần chạy đầu tiên** — trường `description` trong `Cargo.toml` của chính crate telemetry, do tôi viết, câu "Not an audit log". Lời hứa trong tài liệu thì mục ruỗng theo thời gian; cái này thì không.

**Một test đã sống sót mutation và phải viết lại**, đúng vết xe của E2 ở M4: bản đầu của `the_logged_path_identifier_matches_the_key_the_file_carries` lấy giá trị kỳ vọng bằng cách gọi `snapshot.path_id(...)` — tức là so implementation với chính nó. Cho `path_id` tính sẵn băm nhầm chuỗi (`secret/data/app/db` thay vì `app/db`) thì **cả hai vế đổi theo nhau và test vẫn xanh**. Bản hiện tại dẫn `LogKey` thẳng từ token key trong test, và nó giết mutation đó.

**Một lỗi thiết kế bắt được lúc viết:** `log.enabled: false` mà vẫn enqueue thì hàng đợi đầy rồi **mọi dòng bị tính là drop** — tức là tắt log sẽ kéo `kallisto_access_log_dropped_total` lên, đúng cái alarm nghĩa là "có thứ gì đó đang flood tiến trình này". Producer bị tắt giờ không ghi gì cả.

**stdout từ M6 chỉ còn là access log.** Banner khởi động và dòng "loaded version N" chuyển sang stderr, để log shipper đọc stdout như một luồng đúng một hình dạng thay vì một hình dạng lẫn văn xuôi.

**Số đo.** Laptop 8 nhân, 4 worker, wrk2 cùng máy, hai binary **chạy xen kẽ trong cùng một vòng** (bài học M5).

Ở 30k req/s, 4 vòng:

| 30k req/s | M5 | M6 |
| --- | --- | --- |
| p50 | 1.31 / 1.35 / 1.31 / 1.32 ms | 1.32 / 1.36 / 1.31 / 1.32 ms |
| p99 | 3.51 / 3.38 / 3.44 / 3.72 ms | 3.50 / 3.63 / 3.44 / 3.58 ms |

**Không phân biệt được.** Chênh lệch nhỏ hơn dao động giữa các vòng của chính M5 (p99 của nó trải 3.38–3.72).

Ở bão hoà, 8 vòng mỗi bên:

| Bão hoà (mục tiêu 200k) | M5 | M6 |
| --- | --- | --- |
| trung bình | **118.2k** | **111.6k req/s** |
| trung vị | 119.5k | 112.2k |
| min – max | 111.8k – 123.9k | 104.6k – 116.8k |
| độ lệch chuẩn | 5.0k | 5.1k |

**Tụt ~6%.** Ở mức đó, M6 đang ghi ~110 nghìn dòng mỗi giây ra đĩa — **drop 0 dòng**, writer theo kịp hoàn toàn. 6% ấy phần lớn là I/O thật của việc có access log, không phải chi phí của hàng đợi.

**Ca QĐ-8, đo end-to-end.** Bỏ đói writer bằng cách đẩy stdout vào một consumer chỉ đọc ~200 dòng/giây, rồi bắn 60k req/s vào trong 15 giây:

| | |
| --- | --- |
| Request phục vụ | **892.624** |
| Thông lượng | 59.5k req/s |
| p50 / p99 | **1.34 ms / 4.72 ms** |
| Dòng log bị vứt | **881.520** (98,8%) |
| Non-2xx | **0** |

Hàng đợi đầy gần như suốt cả lượt chạy, độ trễ **không đổi**, và `kallisto_access_log_dropped_total` nói đúng con số. Đó chính là câu D15 viết ra để bảo vệ, chứng minh bằng đo chứ không bằng lập luận: **log bỏ cuộc trước, server thì không.**

### M7 — CLI

`cmd/kallisto-ctl` đang là stub `ratatui`. Bỏ TUI (ADR-0004 vẫn `suspended`), đổi binary thành `kallisto-ctl`:

- `seal --in plain.json --out secrets.kal --version N`
- `verify --in secrets.kal` — kiểm tag và số version, không in ra nội dung
- `bump-version --in secrets.kal`
- `validate --config kallisto.yaml` — lời hứa trong phần Confirmation của ADR-0003
- `open` có, nhưng đòi `--yes-print-secrets-to-stdout`

Khoá đọc từ env, không nhận qua tham số dòng lệnh (tham số lộ qua `ps`).

**Đã thi hành.** Năm lệnh theo kế hoạch, cộng **hai lệnh không có trong kế hoạch** — và lý do thêm chúng là chỗ đáng ghi nhất của mốc này:

**`mint-token` đóng một lỗ làm M4 không dùng được.** Khoá của hàng trong map `tokens` là `HMAC-SHA256(token_key, "kallisto/token/v1\0" || token)`. Cái nhãn đứng trước token nghĩa là **`openssl dgst -hmac` không tính ra được giá trị đó** — không có cờ nào nhét thêm tiền tố vào message. Tức là từ M4 tới giờ, bảng token của D8 tồn tại trong code nhưng operator không có cách nào tạo ra một hàng hợp lệ. `mint-token` sinh token ngẫu nhiên 256 bit, in **token ra stdout** (để pipe vào chỗ cất) và **dòng cần dán ra stderr**, nên `mint-token ... > token.txt` lấy đúng token và không lẫn gì khác.

**`gen-key`** in 32 byte từ RNG hệ thống. Nhỏ, nhưng nó gỡ câu "chạy `openssl rand -hex 32`" khỏi tài liệu — một công thức mà người đọc phải tự tin là đúng.

Những chỗ khác đáng ghi:

* **`seal` chặn rollback trước khi ghi.** Seal đè version thấp hơn (hoặc bằng) version đang nằm ở file đích tạo ra một file mà resolver **lặng lẽ từ chối**: bucket nhận upload bình thường, sidecar vẫn phục vụ bản cũ, và không ai biết. Kiểm bằng `peek_version`, không tốn một phép crypto nào và không cần khoá. `--force` là lối thoát khi dựng lại từ đầu.
* **`seal` từ chối file có token mà không có `token_key`** — hình dạng duy nhất resolver vứt thẳng. Nghe từ công cụ vừa ghi file thì hơn là từ một sidecar không chịu khởi động.
* **`bump-version` phải mở file ra rồi seal lại**, không sửa tại chỗ được: số version nằm ở cả header lẫn thân, và header là AAD của AEAD.
* **Thông báo lỗi parse không chuyển tiếp message của `serde_json`** — nó trích dẫn giá trị vừa gây lỗi, mà ở file này giá trị đó là secret. Giữ lại dòng/cột, thêm hình dạng mong đợi, và **nói rõ là đã giấu message gốc cùng lý do**.
* **`--seal-key` và `--token-key` bị từ chối theo tên**, y như server, và thông báo không lặp lại giá trị.

**Một chỗ tôi viết sai và test bắt được ngay:** tài liệu và thông báo của tôi viết `tokenKey` — vì file **config** dùng camelCase kiểu k8s. Nhưng **file secret là JSON dùng `token_key`**, và `deny_unknown_fields` biến chênh lệch đó thành lỗi cứng chứ không phải trường bị bỏ qua. Đã sửa, và thông báo lỗi parse giờ nói thẳng ra sự khác biệt đó.

**Test chạy binary thật**, không phải hàm thư viện: `cmd/kallisto-ctl/tests/round_trip.rs` mint một token bằng chính công cụ, dán hash vào plaintext, seal, rồi **dựng `Snapshot` và khẳng định resolver chấp nhận token đó** — và chỉ cho đúng đường dẫn policy cho phép. Đó là bằng chứng duy nhất rằng `mint-token` đáng có. Ba mutation đã được kiểm: dùng digest trần thay vì keyed hash có nhãn (đúng thứ operator sẽ tự làm bằng `openssl`), so sánh rollback ngược chiều, và `open` quên đòi cờ — mỗi cái bị đúng một test giết.

**`examples/seal_fixture.rs` bị xoá.** Comment của chính nó ghi là nó tồn tại cho tới khi có `kallisto-ctl` ở M7. `run_duck_bench.sh` giờ gieo dữ liệu bằng `kallisto-ctl gen-key` + `seal`, đã chạy lại và vẫn đúng (29.3k req/s ở mục tiêu 30k).

`ratatui` bị gỡ khỏi `Cargo.toml` của crate, sớm hơn M-X một bước — crate được viết lại nguyên nên giữ lại một dependency không ai gọi là vô nghĩa. Package đổi tên `kallisto_tui` → `kallisto-ctl`, binary `kallisto-tui` → `kallisto-ctl`.

### M8 — Bộ test con vịt

Đây là fitness function của cả dự án, không phải phần phụ.

`tests/duck/docker-compose.yml`: Kallisto + Garage (hoặc MinIO) + ba container client chạy **SDK Vault thật** của Go, PHP, Python.

- Ca thuận: mọi dòng ✅ trong bảng D7 phải chạy qua mà SDK không nhận ra khác biệt.
- Ca nghịch: mọi dòng ❌ phải nhận **đúng mã HTTP và đúng thân JSON** Vault trả.
- Ca xấu tường minh: file giả mạo, file cũ bị đặt lại, bucket chết, sai khoá, token đã thu hồi, và **hàng đợi log đầy phải drop mà vẫn phục vụ**.
- Một ca riêng cho pattern đã sinh ra QĐ-3: client lấy secret rồi huỷ ngay, lặp ở rps cao, chạy qua nhiều worker.

Thay thế `tests/e2e_vault_compat.rs` hiện tại — nó test `kv put`/`patch`/`delete`/`undelete`/`destroy`, tức là toàn bộ thứ giờ phải trả 403.

**Đã thi hành. `make duck`: 22 passed.** MinIO thay Garage (image nhỏ hơn, cấu hình một biến môi trường; D2 quan tâm S3-compatible chứ không quan tâm cái nào).

**Mọi container dùng `network_mode: host`.** Ràng buộc vận hành số 1 của ADR là cổng 8200 không bao giờ ra khỏi localhost, và resolver từ chối khởi động ở địa chỉ khác. Dùng bridge network nghĩa là phải *publish* cổng để test nó — tức là test một cách triển khai mà chính ADR cấm. Trên host network, client chạm `127.0.0.1:8200` đúng như một app cùng máy.

**Và đây là phần đáng giá nhất của cả mốc: nó bắt được hai lỗi thật ngay lần chạy đầu, cả hai đều vô hình với 215 test nội bộ.**

* **`GET /v1/sys/init` không tồn tại.** `hvac` gọi nó bên trong `is_initialized()` trước mọi thứ khác, nhận 404, rồi bỏ cuộc ở câu đầu tiên. Không test nào của ta gọi route đó vì ta không biết SDK gọi nó.
* **Vòng refresh nuốt mọi kết quả sau khi khởi động.** `run()` viết `let _ = self.poll_once().await`, nên chỉ hai lần poll lúc boot được báo cáo. Từ giây thứ ba mươi trở đi: file giả mạo, file bị rollback, bucket chết — **không một dòng log nào, không tăng `kallisto_refresh_failures_total`**. Resolver vẫn phục vụ bảng tốt cuối cùng, đúng D14, nhưng **im lặng** — tức là đúng thứ D4 và D14 sinh ra để chống. Ai đó nhét file giả vào bucket mỗi phút sẽ hoàn toàn vô hình. Regression test `the_poll_loop_reports_every_tick_including_the_bad_ones` khẳng định *quan sát* chứ không khẳng định version được phục vụ — vì version được phục vụ luôn đúng suốt thời gian đó, và đó chính là lý do không test nào khác thấy. Đã kiểm bằng cách cài lại đúng bug.

**Ba failure còn lại là lỗi của chính bài test, không phải của sản phẩm** — ghi tách bạch vì gộp chung sẽ làm hồ sơ này nói dối:

* Tôi kiểm 404 bằng đường dẫn **nằm ngoài policy**. Ở đó 403 mới đúng, và cố ý: trả 404 sẽ biến nó thành existence oracle. Sửa test, không sửa server.
* Container PHP resolve dependency theo PHP của image `composer` (8.4) rồi chạy trên `php:8.3-cli`. Thêm `platform` vào `composer.json`.
* Client PHP thiếu PSR-7 implementation, và `csharpru/vault-php` **tự mâu thuẫn**: `read()` cho path qua `buildPath()` (thêm `/v1`), còn `list()` nằm ở lớp cha và gửi path nguyên văn. Để nguyên chỗ lệch đó trong code test kèm comment, vì nó chính là lý do có một client bên thứ ba trong bộ này: nó được viết bằng cách đọc HTTP API của Vault, nên nó giả định những thứ SDK chính thức không giả định.

**Số đo.**

| | |
| --- | --- |
| Tổng | **22 passed, 0 failed** |
| Ca lấy-rồi-huỷ (QĐ-3) | 20000/20000, 2774 req/s, 32 luồng |
| Access log | 20004 dòng, **drop 0** |
| Non-2xx trong ca hot | 0 |

Ca bỏ đói writer (chạy riêng ở M6) vẫn là bằng chứng mạnh hơn cho D15: 892.624 request phục vụ ở p50 1.34 ms trong khi 881.520 dòng log bị vứt.

**Một chỗ build chậm phải sửa:** image PHP ban đầu `apt-get install git unzip` mất hơn 12 phút. Dựng hai stage — `composer:2` đã có sẵn git/unzip, image runtime chỉ cần thư mục `vendor/` — xuống còn **19 giây**.

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

**Đã thi hành: −9.149 dòng, +1.509, 71 file, một commit.** 288 → 214 test; 74 cái mất đi là của engine, và đó là con số đúng chứ không phải mất mát.

Ba chỗ lệch kế hoạch:

* **`src/engine/error.rs` cũng bị xoá**, dù kế hoạch nói giữ. Sau khi E1 được viết lại thì không còn ai dùng `EngineError`, và giữ một kiểu lỗi không ai sinh ra là giữ một file chết.
* **E1 phải viết lại chứ không phải xoá, và đây là chỗ suýt hỏng lặng lẽ.** Chủ thể của E1 là `SecretPayload`, `KeyMetadata`, `EngineError` — toàn bộ thuộc đường ghi. Xoá chúng sẽ mang theo sáu test, và `verification-status.md` sẽ tiếp tục ghi E1 là **Proven** trong khi không còn gì chứng minh. Invariant không đổi; chủ thể của nó đổi. Giờ nó kiểm đúng những kiểu mà secret thật sự đi qua: `Contents` của sealed file, `Snapshot` đang sống, mọi kiểu giữ khoá, và mọi error có thể tới được một dòng log. Cả bốn test redaction đã kiểm bằng cách thay `Debug` viết tay bằng `#[derive(Debug)]`.
* **Hai fuzz target chết theo chủ thể của chúng**, nên đổi mục tiêu chứ không bỏ. `rkyv_roundtrip` mất cùng rkyv; `http_parser` trỏ vào handler engine-era. Thay bằng `sealed_file` — **đầu vào duy nhất mà kẻ tấn công trong mô hình D13 kiểm soát hoàn toàn**, vì mô hình đó *chính là* người ghi được bucket — và `read_path` cho các parser chạy trước mọi quyết định phân quyền.

**Một ranh giới của E1 phải ghi nhận chứ không giấu:** lỗi *kiểu* từ parser config **có** trích dẫn giá trị (`invalid type: string "...", expected usize`). Đó là chỗ duy nhất trong chương trình mà thông báo lỗi lặp lại đầu vào, và là cố ý: file config không chứa secret theo thiết kế (ADR-0003 — khoá và credential không có trường nào để nhét vào, và `deny_unknown_fields` khiến bịa ra một trường là lỗi), còn "dòng 4 sai" mà không nói vì sao thì tốn của operator một giờ. File *secret* được đối xử ngược lại: `kallisto-ctl` giấu message của serde ở đó chính vì giá trị nó vấp phải là secret. Test `e1_configuration_type_errors_quote_the_value_and_that_is_on_purpose` ghim đánh đổi này để nó không bị nhầm là sơ suất.

**`cargo deny` sạch hoàn toàn lần đầu trong lịch sử dự án:** danh sách `ignore` rỗng. Advisory của rkyv 0.7 từng là bài toán *data migration* — archive đã nằm trên đĩa nên sửa nó nghĩa là đổi định dạng. Xoá engine biến nó thành một dòng bị xoá khỏi `deny.toml`, đúng điều D12 dự đoán. Bỏ luôn allowance `MPL-2.0` vì không còn dependency nào mang giấy phép đó.

**`verification-status.md` đánh dấu Retired thay vì xoá** các nhóm đã mất chủ thể (A toàn bộ, B3/B4/B5, C1, D). "Từng proven rồi code biến mất" và "chưa bao giờ proven" là hai sự thật khác nhau, và người đọc có quyền phân biệt.

### Sau M-X — sửa pipeline

Không nằm trong kế hoạch, nhưng thuộc cùng công việc: PR gộp `1.0.0-alpha` vào `main` có năm check đỏ.

**Rust CI chết sau 7 giây, trước khi biên dịch dòng nào:** `'toolchain' is a required input`. `dtolnay/rust-toolchain` suy ra toolchain **từ ref của chính nó**, mà policy supply-chain ghim action bằng SHA → không còn gì để suy ra. Job Miri xanh chỉ vì nó khai báo tường minh.

Chỗ nguy hiểm không phải việc nó đỏ mà là **cách sửa sai sẽ làm nó xanh giả**: đặt `toolchain: stable` thì `rustfmt.toml` — vốn bật `unstable_features` với 10 option nightly-only — bị rustfmt bỏ qua kèm warning rồi **exit 0**. CI xanh trong khi format trôi khỏi thứ mọi dev tạo ra. Giờ mỗi job đọc `rust-toolchain.toml`, một nguồn sự thật duy nhất, không trôi được.

**Docker hỏng hai tầng, và đây là hoá đơn cho việc bỏ qua dòng ở M1** (*"Kiểm cross-compile musl ngay ở mốc này, không để tới lúc dựng Docker"*). Dockerfile được viết ở M-X mà **không build lần nào**:

1. `rustup target add` đứng trước `COPY . .`, nên musl target cài vào stable; `rust-toolchain.toml` tới sau, cargo đổi sang nightly ghim, và ở đó không có musl std → `can't find crate for 'core'`.
2. Sửa xong, vài phút compile sau: `failed to find tool "llvm-ar"`. Package `clang` không kèm nó — phải `llvm`. `aws-lc-rs` là thứ duy nhất trong cây compile C/asm nên không gì khác phát hiện được.

Sau khi sửa và **build tới khi ra image chạy được**: distroless **21.5 MB**, cả `kallisto-server` lẫn `kallisto-ctl` chạy bên trong — distroless không có libc, nên chạy được nghĩa là link tĩnh thật. D12 từ ý định thành đã kiểm chứng.

**GitGuardian nói đúng:** `tests/duck` commit user/password MinIO tĩnh. Giờ sinh từ `/dev/urandom` mỗi lần chạy. Fixture password đổi tên trên toàn workspace — một repo dạy reviewer bỏ qua chuỗi hình dạng mật khẩu là đã **vô hiệu hoá scanner chứ không phải thoả mãn nó**. Test vector SigV4 của AWS tách qua `concat!`, và `.gitguardian.yaml` tham chiếu bằng **sha256 chứ không bằng giá trị** — viết một chuỗi hình dạng AWS key vào file ignore thì chỉ là dời cái hit vào đó.

**Bốn alert CodeQL đều là false positive**, dismiss kèm lý do. Hai cái *critical* nói có nonce AES-GCM hard-code: một là `[0u8; 12]` bị `copy_from_slice` ghi đè bằng header của file — **nonce đang được đọc**, không phải sinh ra — cái kia được `SystemRandom::fill` lấp ngay dòng sau, mà CodeQL không mô hình hoá aws-lc-rs là nguồn ngẫu nhiên.

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
| M5 (có barrier trên đường nóng) | mức tụt phải đo được và phải báo cáo | ✅ xem mục M5 |
| M6 (có access log trên đường đọc) | mức tụt phải đo được; hàng đợi đầy không được làm tụt thông lượng | ✅ xem mục M6 |
| M8 | chạy ca lấy-rồi-huỷ ở rps cao qua nhiều worker | ✅ 20000/20000 ở 2774 req/s, 0 non-2xx — xem mục M8 |

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
- **aws-lc-rs cần `cmake` + `clang` lúc build.** Nhẹ hơn RocksDB nhiều nhưng không phải bằng không; cross-compile musl phải kiểm ở M1. — *Đã xảy ra đúng như vậy, và còn cần `llvm` cho `llvm-ar` nữa, thứ không ai đoán trước được. Rủi ro này được ghi ra rồi vẫn bị bỏ qua.*
- **Ngân sách vượt ~400 dòng** so với trần 2.000 của ADR, do config, hardening và lớp worker được giữ lại chưa được tính vào trần đó.
