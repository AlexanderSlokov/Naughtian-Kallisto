# Thử Kallisto bản mới

Bản sau "duck turn" (ADR-0015/0016) **khác hẳn** bản cũ: không còn engine, không còn
RocksDB, không còn cổng 8202, và **mọi thao tác ghi đều trả 403**. Nó đọc secret từ
**một file mã hoá trên bucket S3-compatible**, phục vụ qua API Vault KV-v2 trên
`127.0.0.1:8200`.

Dưới đây là đường đi ngắn nhất để tự kiểm chứng.

---

## 0. Chuẩn bị

```bash
# Cần: Rust 2024 stable, cmake, clang (cho aws-lc-rs — dependency C duy nhất còn lại)
sudo apt-get install -y cmake clang

git checkout duckilized
make build
```

Nếu muốn chạy bài test con vịt ở mục 4 thì cần thêm `docker` + `docker compose`.

---

## 1. Chạy thử trong 2 phút (không cần bucket)

Nguồn `disk` đọc file ngay trên máy — đủ để thấy toàn bộ hành vi.

```bash
# Khoá seal. KHÔNG bao giờ truyền qua tham số dòng lệnh (lộ qua `ps`),
# công cụ sẽ từ chối nếu bạn thử.
export KALLISTO_SEAL_KEY=$(cargo run -q -p kallisto-ctl -- gen-key)

# File plaintext. Chú ý: token_key (snake_case), KHÔNG phải tokenKey.
cat > /tmp/plain.json <<'JSON'
{
  "version": 1,
  "secrets": {
    "app/db":  {"username": "admin", "password": "duck-fixture-not-a-credential"},
    "app/sub/deep": {"k": "v"}
  },
  "policies": {},
  "tokens": {}
}
JSON

cargo run -q -p kallisto-ctl -- seal --in /tmp/plain.json --out /tmp/secrets.kal

cat > /tmp/kallisto.yaml <<'YAML'
apiVersion: kallisto/v1
kind: Resolver
spec:
  listen: { address: 127.0.0.1, port: 8200 }
  workers: 2
  mount: secret
  source: { type: disk, path: /tmp/secrets.kal }
  refresh: { intervalSeconds: 5 }
YAML

cargo run -q -p kallisto-server -- --config /tmp/kallisto.yaml
```

Terminal khác:

```bash
export VAULT_ADDR=http://127.0.0.1:8200

# Đọc — chạy được
vault kv get secret/app/db
vault kv list secret/app

# Ghi — PHẢI là 403. Đây là sản phẩm, không phải thiếu tính năng.
vault kv put secret/app/db x=y        # => permission denied
vault kv delete secret/app/db         # => permission denied

# Trạng thái thật (không hardcode như bản cũ)
curl -s localhost:8200/v1/sys/health | jq
curl -s localhost:8200/v1/sys/metrics | grep kallisto_
```

Ở terminal chạy server bạn sẽ thấy **access log** trên stdout: mỗi request một dòng,
**đường dẫn và token đã băm có khoá**, không bao giờ có giá trị secret.

---

## 2. Thử phân quyền bằng token

```bash
TOKEN_KEY=$(cargo run -q -p kallisto-ctl -- gen-key)

# mint-token in token ra stdout, dòng cần dán ra stderr
TOKEN=$(KALLISTO_TOKEN_KEY=$TOKEN_KEY cargo run -q -p kallisto-ctl -- \
        mint-token --policy app 2>/tmp/mint.txt)
cat /tmp/mint.txt          # <- lấy chuỗi hash trong này
echo "token = $TOKEN"      # <- chỉ hiện MỘT lần, mất là phải mint lại
```

Dán hash vào `/tmp/plain.json`:

```json
  "policies": { "app": [
      {"path": "secret/data/app/*",     "capabilities": ["read"]},
      {"path": "secret/metadata/app/*", "capabilities": ["list","read"]}
  ]},
  "tokens": { "<hash vừa lấy>": ["app"] },
  "token_key": "<TOKEN_KEY>"
```

Rồi **tăng version** (resolver từ chối file cũ hơn cái nó đang giữ):

```bash
cargo run -q -p kallisto-ctl -- seal --in /tmp/plain.json --out /tmp/secrets.kal --version 2
```

Đợi ~5 giây rồi thử:

```bash
VAULT_TOKEN=$TOKEN vault kv get secret/app/db    # OK
VAULT_TOKEN=s.saibet vault kv get secret/app/db  # 403
vault kv get secret/app/db                       # 403 (không token)
```

**Thu hồi token** = xoá dòng đó khỏi `tokens`, tăng version, seal lại. Không có API revoke.

---

## 3. Công cụ offline `kallisto-ctl`

```bash
cargo run -q -p kallisto-ctl -- help
```

| Lệnh | Việc |
| --- | --- |
| `gen-key` | 32 byte từ RNG hệ thống |
| `seal --in plain.json --out f.kal [--version N]` | Mã hoá. **Từ chối** ghi version mà resolver sẽ coi là rollback |
| `verify --in f.kal` | Kiểm tag, in số lượng. Không bao giờ in secret hay đường dẫn |
| `bump-version --in f.kal [--to N]` | Tăng version tại chỗ |
| `mint-token --policy <tên>` | Sinh token + dòng để dán |
| `validate --config kallisto.yaml` | Parse & resolve, in ra server sẽ làm gì |
| `open --in f.kal --yes-print-secrets-to-stdout` | In hết ra. Đúng như tên gọi |

Thử vài ca xấu cho vui:

```bash
cargo run -q -p kallisto-ctl -- seal --in /tmp/plain.json --out /tmp/secrets.kal --version 1
#   => từ chối: file đang giữ version 2

printf '\x01' | dd of=/tmp/secrets.kal bs=1 seek=40 conv=notrunc status=none
cargo run -q -p kallisto-ctl -- verify --in /tmp/secrets.kal
#   => failed authentication
```

---

## 4. Bài test con vịt (`make duck`) — cái đáng tin nhất

Đây là **fitness function** của cả dự án: Kallisto thật + MinIO thật + **ba SDK Vault
thật** (Go của HashiCorp, `hvac` của Python, `csharpru/vault-php`), cộng các ca hỏng.

```bash
make duck
```

Lần đầu mất vài phút để build 3 image client; sau đó ~3 phút. Kết quả hiện tại:

```
duck: 22 passed
```

Nó kiểm: 3 SDK đọc/list/metadata bình thường, mọi đường ghi trả **đúng 403 và đúng
thân JSON của Vault**, phân quyền, token bị thu hồi, file giả mạo, file cũ bị đặt lại,
sai khoá, **bucket chết vẫn phục vụ**, **restart lúc bucket chết vẫn khởi động được từ
bản cache mã hoá trên đĩa**, và ca lấy-rồi-huỷ ở rps cao.

> Bài test này đã bắt được **2 lỗi thật** ngay lần chạy đầu — xem mục 6.

---

## 5. Các gate còn lại

```bash
make dev       # fmt + clippy + cargo-deny + test   (215 test)
make verify    # bộ verification chặn PR của ADR-0013
make bench-duck
cargo bench --bench barrier_bench
```

`cargo deny` giờ **sạch hoàn toàn** — danh sách `ignore` rỗng lần đầu tiên trong lịch
sử dự án, vì advisory của rkyv biến mất cùng engine.

---

## 6. Hai lỗi thật mà `make duck` tìm ra

Ghi lại vì đây chính là lý do bài test này tồn tại — cả hai đều **không** test unit nào
bắt được:

1. **`GET /v1/sys/init` không tồn tại.** `hvac` gọi nó trong `is_initialized()` trước
   mọi thứ khác, và nhận 404 → client bỏ cuộc ngay câu đầu tiên. Đã thêm.

2. **Vòng refresh nuốt mọi lỗi sau khi khởi động.** `run()` viết
   `let _ = self.poll_once().await` — hai lần poll lúc boot thì có báo cáo, còn **mọi
   lần sau đều bị vứt**: file giả mạo, file bị rollback, bucket chết đều không sinh
   dòng log nào và không tăng `kallisto_refresh_failures_total`. Resolver vẫn phục vụ
   bảng tốt cuối cùng (đúng D14) nhưng **im lặng**, tức là đúng thứ D4/D14 sinh ra để
   chống. Ai đó nhét file giả vào bucket mỗi phút sẽ hoàn toàn vô hình.
   Đã sửa, và có regression test `the_poll_loop_reports_every_tick_including_the_bad_ones`
   — đã kiểm bằng cách cài lại đúng cái bug đó và xem nó fail.

Ngoài ra 3 lỗi còn lại là **lỗi của chính bài test**, không phải của sản phẩm: tôi
dùng đường dẫn nằm ngoài policy để kiểm 404 (đúng ra phải là 403 — và 403 ở đó là cố
ý, để không thành existence oracle), và cấu hình container PHP sai.

---

## 7. Những thứ KHÔNG chống được — đọc trước khi tin

- **root trên máy**, hoặc ai đọc được RAM tiến trình. Barrier chỉ làm core dump/swap
  bớt giá trị, không chặn debugger đang chạy.
- **Thân response đang bay** là chữ trần. Đó là bản chất của việc phục vụ secret.
- **Ai giữ seal key** đọc được file.
- **Access log KHÔNG phải audit log.** Hàng đợi đầy thì nó vứt dòng để giữ cho máy còn
  phục vụ được, nên nó **không** trả lời chắc chắn được "ai đã đọc Stripe key".
  `kallisto_access_log_dropped_total` cho biết khi nào điều đó xảy ra — hãy đặt alert.

Hai luật vận hành:

1. **Không bao giờ mở cổng 8200 ra ngoài localhost.** Server từ chối khởi động nếu bind
   địa chỉ khác mà không có `--i-accept-the-risk`.
2. **App phải đọc secret lúc runtime.** Đừng nướng vào cache của framework — Laravel
   `config:cache` ghi thẳng secret ra `bootstrap/cache`, và làm thế là mất sạch ý nghĩa
   của việc có resolver.
