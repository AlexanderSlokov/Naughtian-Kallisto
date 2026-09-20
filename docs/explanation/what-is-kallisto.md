# Naughtian Kallisto là gì?

Kallisto là một secrets resolver **chạy cục bộ và chỉ đọc**, nói giao thức Vault KV-v2 trên **một
file đã mã hoá** nằm trên bucket tương thích S3. Nó chạy cạnh ứng dụng của bạn trên localhost,
nghe ở cổng 8200 và chỉ trên loopback.

Nó không phải một secrets server. Nó không lưu trữ, không nhân bản, không có cluster, không có
database, không có admin API. **Mọi route ghi đều trả 403** — đó là sản phẩm, không phải tính năng
chưa làm. Một resolver không ghi được là một resolver mà credential của nó vô giá trị với kẻ đánh
cắp được.

Đổi sang dùng nó là một dòng:

```diff
- VAULT_ADDR=https://vault.internal:8200
+ VAULT_ADDR=http://127.0.0.1:8200
```

Bỏ nó đi cũng là một dòng đó. Nếu ứng dụng của bạn dùng Vault SDK, nó không nên nhận ra khác biệt.

## Cách nó hoạt động

```
  người vận hành                bucket                     mỗi máy
  ──────────────                ──────                     ───────
  kallisto-ctl seal  ──────►  secrets.kal  ──────►  kallisto-server :8200
   (AES-256-GCM,              (đã mã hoá,           (poll, xác thực, từ chối
    version N)                 có version)           rollback, phục vụ đọc)
                                                            │
                                                     app ───┘  VAULT_ADDR=127.0.0.1
```

Mọi thao tác *ghi* ra file sealed đều xảy ra offline trong `kallisto-ctl`, không bao giờ qua mạng.
File mang theo secret, policy, và bảng hash token có khoá — bảng policy được mã hoá cùng mọi thứ
khác, vì quyền ghi vào bucket và việc giữ khoá là hai chuyện khác nhau.

Server poll file mỗi 30 giây theo mặc định. Chưa có file nào nạp được thì đó chính là trạng thái
*sealed* của Vault: trả 503, không phải 200 rỗng. File hỏng hay bị giả mạo thì bảng cũ **giữ
nguyên tại chỗ**. Bucket chết thì máy vẫn phục vụ từ bản sao đã mã hoá trên đĩa cục bộ.

Chi tiết từng tầng nằm ở [architecture.md](./architecture.md).

## Nó không phải bản thay thế Vault

Không có auth method, dynamic secret, lease, PKI, transit engine. Nếu bạn cần những thứ đó, hãy
chạy OpenBao và đặt Kallisto phía trước nó. Kallisto đọc **một** file, trên bucket bạn chọn: S3,
R2, MinIO, SeaweedFS, RustFS, hay một cluster Garage.

Loại secret nào nên và không nên đặt ở đây là một câu hỏi riêng, có bảng quyết định ở
[why-use-naughtian-kallisto/use-cases.md](./why-use-naughtian-kallisto/use-cases.md).

## Vì sao nó nhanh, và nhanh tới đâu

Đường đọc là đường nóng, nên nó được xây như đường nóng: một Tokio runtime `current_thread` cho
mỗi worker, ghim vào core, nhiều worker chung một cổng qua `SO_REUSEPORT`. Không work-stealing,
không dynamic dispatch trên đường đọc — handler nạp snapshot hiện tại rồi đọc một `HashMap`.

Đo trên laptop 8 core với 4 worker: khoảng **99k req/s** ở mức bão hoà, và **p50 1.3 ms / p99
3.5 ms** ở 30k req/s. Lý do vì sao đường đọc được coi là đường nóng, cùng toàn bộ số đo, ở
[why-use-naughtian-kallisto/serving-kv-secrets.md](./why-use-naughtian-kallisto/serving-kv-secrets.md).

## Nó từng là gì

Đây là kiến trúc thứ ba. Hai bản trước — một lõi C++ với FFI bridge, rồi một secrets *server*
Rust thuần có cuckoo table, B-tree index, RocksDB và control plane gossip — đều đã bị xoá. ADR-0015
đổi câu hỏi từ "làm sao xây một secrets server nhanh" thành "một máy thì ứng dụng của nó thực sự
cần gì", và câu trả lời nhỏ hơn rất nhiều: khoảng chín nghìn dòng bị lấy ra.

Vì sao hai bản kia bị bỏ, và điều đó định hình bản này thế nào:
[architecture-in-deep.md](./how-to-create-naughtian-kallisto/architecture-in-deep.md).

## Trạng thái

Prototype đang được làm lại. Chưa dùng được cho production. Bản 1.x không cam kết ổn định API.
Giấy phép AGPLv3.
