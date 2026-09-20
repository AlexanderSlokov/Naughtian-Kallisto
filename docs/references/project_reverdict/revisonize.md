# What the fuck is this project?

1. Tôi cũng không biết nữa. Ban đầu, tôi muốn nó là một cái Vault, nhưng nhanh như Redis,
chạy với kiến trúc của Envoy. Nó chạy nhanh khủng khiếp thời nó còn là C++, đạt 
120k RPS trên cái con HP Pavilion 15 tội nghiệp này. Một cái "Vault siêu tải", nhanh 
như gió.
2. Xong, tôi nghe rằng code state machine rất khổ cực và không an toàn, thế là tôi
lại định hướng cái project này làm Dataplane cho Roots of Trust.
3. Tôi ban đầu cũng muốn nó monolith, nhưng rồi nhìn sang Vault thấy nó là Hexagonal,
lại quay xe làm hexagonal, chịu thiệt hại 25% thông lượng. Xong cuối cùng là chuyển sang
Rust, bay thêm 20% thông lượng nữa là chỉ còn lèo tèo 72k RPS.
4. Giờ thì đòi hỗ trợ cả API gateway, config client cho các config server, rồi đòi
làm chỗ chứa env an toàn cho app / pod này nọ.
5. Eventual consistency? Với foca? Trong khi bản thân nó còn chưa biết nó là cái mẹ gì?
6 Mang tiếng là cũng muốn làm Encrypt Barrier đồ he, nhưng thực thế thì KEK phải nhờ
Vault giữ, nhận cấu hình ACL và Realm cũng phải từ Vault.
6. Ủa vậy rốt cuộc cái này là cái gì? Cache không ra Cache, vault không ra secret Vault,
Config server thì đéo, Key management thì không ra Key management...
7. Nó không nhanh, nó không an toàn, nó không ổn định, nó không có gì tốt cả.

Rồi giờ sao?

---

## Trả lời thẳng, sau khi đọc hết docs, README, codebase

### Bạn đã biết nó là cái gì rồi

README dòng đầu tiên:

> *"Naughtian Kallisto is a High-Performance Secret Dataplane built with Rust,
> designed for high-throughput and low-latency reads."*

`use-cases.md` phân biệt rạch ròi: operational secrets (Kallisto) vs. compliance
secrets (Vault). Có bảng "Good Fit" và "Do NOT Store" cực kỳ rõ ràng. Có cả
decision rule: *"If this secret leaks and I revoke it within 5 minutes, is the
damage contained?"* — Yes → Kallisto, No → Vault.

Bạn thậm chí còn vẽ cả architecture diagram trong docs: Vault bên trái (Root of Trust,
~500 RPS, full audit), Kallisto bên phải (Operational KV, ~36k RPS/core, low latency),
microservices ở dưới gọi lên.

**Bạn không hề "không biết nó là cái gì".** Bạn biết rất rõ. File này là lúc bạn
nghi ngờ chính mình, không phải lúc bạn lạc hướng.

### Những gì bạn tự chê — nhìn lại cho công bằng

**"Nó không nhanh"** — 72k RPS sau khi chịu hai lần thiệt hại (hexagonal -25%,
Rust rewrite -20%), trên laptop. Vault OSS được ~5-8k RPS. Bạn nhanh hơn gần
10 lần. 91k+ RPS với GET p99 = 2.63ms ở thời kỳ đỉnh cao. Cái này không phải
"không nhanh". Cái này là rất nhanh.

**"Nó không an toàn"** — KEK delegate cho Vault Transit, DEK encrypt qua
AES-256-GCM, `zeroize` + `secrecy` wrapper, KEK không bao giờ ghi xuống đĩa.
Đây là envelope encryption chuẩn, đúng theo mô hình mà AWS KMS, GCP CMEK đều
dùng. Bạn không tự giữ master key — đó là **thiết kế đúng**, không phải thiếu sót.

**"Cache không ra cache"** — Đúng rồi, vì nó không phải cache. Nó là persistent
secret store với in-memory hot path. Write-behind queue + RocksDB WAL = dữ liệu
không mất khi restart. Cache thuần (Redis) mất hết khi crash. Kallisto thì không.

**"KEK phải nhờ Vault giữ"** — README chính bạn viết:

> *"Please keep in mind that Naughtian Kallisto can not run by itself and should
> be integrated into existing secret management systems. This is an intentional
> design decision."*

Đây là feature. Bạn đã tự thiết kế nó như vậy. Giờ bạn lại tự chê nó vì điều
bạn chủ đích làm.

### Những gì thật sự cần lo lắng

Thay vì crisis về identity (đã rõ ràng), hãy lo về những thứ thật sự chưa xong:

**1. Encrypt Barrier chưa hoạt động** — Key hierarchy đã thiết kế
(`Vault Master Key → KEK → DEK → AES-256-GCM → RocksDB`) nhưng chưa implement.
`vault_client.rs`, `keyring.rs`, `dek.rs` đều trong roadmap, chưa xong. Không có
encrypt-at-rest thì không ai tin để dùng production.

**2. ACL chưa có** — Không có token-based auth, không có path-based policy. Bất kỳ
ai connect được tới port 8200 đều đọc hết mọi secret. Đây là blocker lớn nhất cho
bất kỳ use case production nào.

**3. TLS chưa có** — Data plane port 8200 đang chạy plain HTTP. Secret truyền qua
mạng không mã hóa. Dùng standalone thì OK, nhưng muốn làm DaemonSet trên node thì
phải có ít nhất TLS, tốt nhất là mTLS.

**4. Config file chưa có** — Mọi thứ qua CLI args. Production deployment cần file
config (`kallisto.hcl` hoặc `kallisto.yaml`) với validation, env var override, v.v.

**5. Gossip cluster (`foca`) chưa rõ vai trò** — Mount engine gì qua gossip?
Invalidate cache ra sao? Sync metadata hay sync data? Eventual consistency cho
secret store là câu chuyện phức tạp cần thiết kế kỹ trước khi code.

### Về "API Gateway" và "Config Server"

Nhìn lại, docs chính thức liệt kê "Secure edge config server" là use case hợp lệ —
phục vụ shared TLS certs, API keys cho gateway/LB fleet. Đây không phải scope creep,
đây là một dạng khác của "serving operational secrets", chỉ khác người dùng cuối
(infra components thay vì microservices). Miễn là nó vẫn dùng cùng KV engine + API
thì không có gì sai.

Nhưng **"API Gateway" theo nghĩa routing/rate-limiting/load-balancing** thì đúng là
không phải việc của Kallisto. Nếu ý bạn ở điểm 4 là vậy thì bỏ.

### Tóm lại

Bạn không lạc hướng. Bạn mệt. Đây là dự án một người, đã đi qua 6 phase,
rewrite từ C++ sang Rust, port Hexagonal Architecture, đạt Vault KV v2 API
compliance, build lock-free queue, sharded cuckoo hashing, write-behind flusher.
Trên một con HP Pavilion 15.

Identity đã rõ từ ngày bạn viết README:

> **Naughtian Kallisto = High-Performance Secret Dataplane for Roots of Trust.**

Cái thiếu không phải là identity. Cái thiếu là encrypt barrier, ACL, TLS, và
config — bốn thứ cụ thể, đo đếm được, giải quyết được. Không phải khủng hoảng
hiện sinh.

Nghỉ đi. Rồi quay lại làm `vault_client.rs`.

---

## Hỏi thêm: Gossip, Master Key, và Consul

### 1. Có nên cluster với Gossip như Cassandra không?

**Không nên replicate data qua gossip.** Lý do:

Cassandra dùng gossip cho **membership + token ring metadata**, không phải để
replicate data. Data replication của Cassandra chạy qua protocol riêng (hinted
handoff, read repair, anti-entropy repair) — hoàn toàn tách biệt khỏi gossip.

`foca` (thư viện gossip hiện tại trong `kallisto_cluster`) là SWIM protocol —
thiết kế cho **failure detection + membership**, không phải data replication.
Đúng tool, sai kỳ vọng nếu bạn muốn dùng nó để sync secret data.

**Gossip nên dùng cho:**

| Dùng gossip | Không dùng gossip |
|---|---|
| Membership: ai đang sống, ai đã chết | Replicate secret values giữa các node |
| Cache invalidation: "key X vừa bị xóa/update, invalidate đi" | Full data sync |
| Cluster metadata: engine mount config, node roles | Write-path coordination |
| Health/load broadcasting | Consistency guarantees |

**Mô hình hợp lý cho Kallisto cluster:**

```
                    ┌─────────────────────┐
                    │   Vault / OpenBao   │
                    │   (Source of Truth)  │
                    └──────────┬──────────┘
                               │ 
                    ┌──────────▼──────────┐
                    │  Sync / Pull Agent  │
                    │  (per Kallisto node) │
                    └──┬──────────────┬───┘
                       │              │
              ┌────────▼───┐    ┌─────▼────────┐
              │ Kallisto-1 │◄──►│ Kallisto-2   │
              │ (node A)   │    │ (node B)     │
              └────────────┘    └──────────────┘
                     ▲  gossip: membership,
                     │  invalidation signals,
                     │  NOT data replication
```

Mỗi node Kallisto tự pull data từ Vault (hoặc nhận push từ Vault agent).
Gossip chỉ dùng để broadcast: "key `secret/db-password` vừa bị rotate ở
node A, các node khác invalidate cache entry đó đi". Node nhận signal thì
tự fetch lại từ Vault khi có request tiếp theo (lazy reload).

Nếu muốn sync data giữa các Kallisto node mà **không qua Vault**, thì đó
là bài toán distributed consensus, cần Raft hoặc tương đương. Nhưng khi đó
bạn đang build lại một nửa Vault. Đừng.

### 2. Standalone mode: Master Key tự tạo hay phải nhờ Vault?

Nhìn vào code: `kallisto_crypto/src/` có `master_key.rs`, `shamir.rs`,
`rotation.rs` — **tất cả đều rỗng**. README của component thì viết đặc tả
Shamir rất chi tiết (GF(2^8), Lagrange interpolation, constant-time ops).
Nhưng roadmap lại ghi:

> *~~Shamir's Secret Sharing~~: ~~**ĐÃ HỦY**~~ → **KHÔI PHỤC (14/08/2026)** — Sẽ implement cả hai mode: Vault Transit (auto-unseal) + Standalone Shamir (manual unseal).*

~~**Mâu thuẫn.** Đặc tả Shamir vẫn nằm đó, code rỗng, roadmap nói đã hủy.~~
**Đã giải quyết (14/08/2026).** Quyết định: implement cả hai con đường.
Vault Transit trước (Phase 3a), Shamir sau (Phase 3b). Lý do bổ sung:
có unseal key standalone thì test encrypt barrier dễ hơn gấp bội so với
phải dựng Vault instance mỗi lần chạy test.

Đây là hai con đường:

---

**Con đường A: Kallisto tự giữ Master Key (Standalone Sovereign)** → *SẼ LÀM (Phase 3b)*

```
Lúc init:
  1. Kallisto sinh Master Key 256-bit từ /dev/urandom
  2. Cắt bằng Shamir (5 shares, threshold 3)
  3. In 5 unseal keys ra stdout (một lần duy nhất)
  4. Master Key encrypt Keyring → ghi Keyring mã hóa xuống đĩa
  5. zeroize Master Key khỏi RAM

Lúc restart:
  1. Kallisto ở trạng thái "sealed" — từ chối mọi request
  2. Operator nạp 3/5 unseal keys qua API hoặc CLI
  3. Shamir combine → khôi phục Master Key
  4. Decrypt Keyring → Kallisto "unsealed", bắt đầu phục vụ
```

**Ưu điểm:**
- Chạy độc lập hoàn toàn, không cần Vault
- Phù hợp edge deployment, air-gapped, hoặc dev/staging
- Giống UX của Vault, người dùng quen thuộc
- **Test encrypt barrier không cần external dependency**

**Nhược điểm:**
- Phải tự implement Shamir đúng (constant-time, GF(2^8), zeroize)
- Manual unseal mỗi lần restart → chậm, cần người trực
- Nếu mất 3/5 unseal keys → mất hết data, không recover được

---

**Con đường B: Delegate cho Vault Transit (Current Roadmap)** → *LÀM TRƯỚC (Phase 3a)*

```
Lúc startup:
  1. Kallisto authenticate với Vault (AppRole / K8s auth)
  2. Gọi POST /v1/transit/decrypt/kallisto-kek
  3. Vault trả về KEK (đã unwrap)
  4. Kallisto giữ KEK in-memory (zeroize on drop)
  5. KEK decrypt DEK → Kallisto unsealed, tự động

Không cần Shamir. Không cần manual unseal.
```

**Ưu điểm:**
- Auto-unseal: restart không cần người
- Master Key không bao giờ rời Vault (HSM-backed nếu muốn)
- Ít code crypto tự viết → ít bug surface
- Key rotation: gọi Vault API, không cần re-seal

**Nhược điểm:**
- Phụ thuộc Vault lúc startup (nếu Vault chết → Kallisto không khởi động được)
- Cần network access tới Vault

---

**~~Khuyến nghị~~ Quyết định (14/08/2026): Làm cả hai, theo thứ tự.**

1. **Phase 3a — Vault Transit (auto-unseal):** Implement trước vì ít code hơn,
   và phù hợp production use case chính (Kallisto as dataplane cho Vault).
   Đây là `vault_client.rs` + `keyring.rs` + `dek.rs`.

2. **Phase 3b — Standalone Shamir (manual unseal):** Implement sau khi Vault Transit
   đã hoạt động và được test. Dùng cho edge/standalone deployment. Lúc này đã
   có Keyring + DEK logic hoạt động, chỉ cần thay source của Master Key từ Vault
   sang Shamir combine. **Bonus: có unseal key cứng để cắm vào integration test.**

3. **Đừng giết con Vault nào cả.** Ý tưởng "dựng Vault tạm, init 5 Shamir keys,
   rồi giết Vault xấu số" là over-engineering. Nếu muốn standalone thì Kallisto
   tự sinh Master Key + Shamir, không cần Vault trung gian.


### 3. Chơi với Consul, Nacos? Chi?

**Không đá bát cơm Consul.** Nhưng cũng không chơi cùng Consul.

Consul và Nacos là **config/service discovery** — chứa config thường (feature
flags, endpoint URLs, retry policies). Dữ liệu này **không nhạy cảm** về mặt
bảo mật, cần **consistency** (Raft consensus), và cần **watch/subscribe** để
hot-reload.

Kallisto là **secret store** — chứa dữ liệu **nhạy cảm** (tokens, passwords,
TLS certs, API keys). Cần **encryption at rest**, **access control**, **audit
trail**, và **tốc độ đọc cực cao**.

| | Consul/Nacos | Kallisto |
|---|---|---|
| **Chứa gì** | Config thường, service registry | Secrets, credentials, keys |
| **Mã hóa** | Không (hoặc tùy chọn) | Bắt buộc (AES-256-GCM) |
| **Consistency** | Strong (Raft) | Eventual hoặc single-node |
| **Tốc độ** | Vừa đủ (~10k RPS) | Cực cao (~72k+ RPS) |
| **Watch/Subscribe** | Có | Không cần |
| **Ai dùng** | App đọc config | App đọc secret để auth/encrypt |

**Câu trả lời cho "Kallisto giữ config key quá quan trọng mà không dám nhét
vào Consul":** Đúng, chính xác là vậy.

Ví dụ thực tế:

```yaml
# Nhét vào Consul (config thường, không nhạy cảm):
service.payment.timeout: 30s
service.payment.retry_count: 3
feature.new_checkout_flow: true
service.payment.endpoint: https://payment.internal:443

# Nhét vào Kallisto (secret, nhạy cảm):
service.payment.api_key: sk_live_xxxxx       # ← stripe secret key
service.payment.tls_cert: |                   # ← mTLS cert
  -----BEGIN CERTIFICATE-----
  MIIBxTCCAWugAwIBAgIJAL...
database.payment.password: "P@ssw0rd!123"     # ← DB credential
jwt.signing_key: "hmac-sha256-key-here"       # ← session signing
```

Consul cho config, Kallisto cho secret. Không ai đá bát cơm ai. Hai bài toán
khác nhau, hai tool khác nhau, cùng phục vụ một hệ thống.

**Nếu muốn tích hợp:** Kallisto có thể register vào Consul như một service
(`kallisto-dataplane`) để các microservices discover Kallisto endpoint qua
Consul DNS/API. Nhưng Kallisto không đọc/ghi config từ Consul, và Consul
không đọc/ghi secret từ Kallisto.