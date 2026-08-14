# Nếu tôi là System Architect, tôi sẽ đặt Kallisto ở đâu?

> Viết bởi AI, dựa trên toàn bộ thông tin về Kallisto đã đọc.
> Giả định: Encrypt Barrier, ACL, TLS đã hoạt động (version 2.0.0+).

---

## Bối cảnh: Vấn đề mà Vault không giải được

Vault là source of truth tuyệt vời. Nhưng trong production ở scale lớn,
có một vấn đề mà mọi team đều gặp:

**Vault chậm, và nó chậm by design.**

Vault dùng Raft consensus (hoặc Consul backend) cho consistency. Mỗi read
phải đi qua storage backend, decrypt, check ACL, audit log. Kết quả:
~5-8k RPS trên phần cứng tốt. Tăng read throughput = thêm Vault node +
load balancer + performance standby. Chi phí vận hành tăng tuyến tính.

Khi microservices fleet của bạn có 200 services, mỗi service đọc 5 secrets
mỗi 30 giây để rotate → đó đã là ~33 requests/second chỉ cho rotation.
Thêm cold start, reconnect, retry → dễ dàng đạt 500-2000 RPS chỉ cho
secret reads. Vault xử lý được, nhưng bạn bắt đầu thấy p99 tăng.

Bây giờ scale lên 2000 services. Hoặc thêm 5 regions. Vault cluster
trở thành bottleneck. Đây là lúc Kallisto có ý nghĩa.

---

## Scenario 1: Kubernetes DaemonSet — "Vault trên mỗi Node"

### Vấn đề

Trong Kubernetes cluster, mỗi pod cần secrets (DB password, API keys,
TLS certs). Có 3 cách phổ biến hiện tại:

1. **K8s Secrets** — base64, không encrypt-at-rest (trừ khi bật KMS),
   mọi pod trên node đều đọc được nếu có RBAC lỏng.
2. **Vault Agent Sidecar** — mỗi pod có một sidecar container chạy
   Vault Agent. Nặng: +50MB RAM/pod, +1 vCPU/pod khi busy.
3. **Vault CSI Provider** — mount secret vào volume. Chậm khi rotate,
   không phù hợp cho secrets thay đổi thường xuyên.

### Giải pháp: Kallisto DaemonSet

```
┌─── Kubernetes Node ───────────────────────────────┐
│                                                    │
│  ┌─────────────┐  ┌─────────────┐  ┌───────────┐  │
│  │  Pod A      │  │  Pod B      │  │  Pod C    │  │
│  │  (payment)  │  │  (auth)     │  │  (order)  │  │
│  └──────┬──────┘  └──────┬──────┘  └─────┬─────┘  │
│         │                │               │         │
│         └────────────────┼───────────────┘         │
│                          │                         │
│                 localhost:8200                      │
│                          │                         │
│              ┌───────────▼───────────┐             │
│              │   Kallisto DaemonSet  │             │
│              │   (1 per node)       │             │
│              │                      │             │
│              │ • 72k RPS capacity   │             │
│              │ • Encrypt-at-rest    │             │
│              │ • ACL per service    │             │
│              │ • ~2ms p99 GET       │             │
│              └───────────┬──────────┘             │
│                          │                         │
└──────────────────────────┼─────────────────────────┘
                           │ TLS (startup + rotation sync)
                           ▼
                 ┌───────────────────┐
                 │  Vault Cluster    │
                 │  (control plane)  │
                 └───────────────────┘
```

### Tại sao tốt hơn

| | Vault Sidecar | Kallisto DaemonSet |
|---|---|---|
| **RAM overhead** | +50MB × N pods | +50MB × 1 per node |
| **Network** | Mỗi pod → Vault (qua network) | Pod → localhost (loopback) |
| **Latency** | 10-50ms (network + Vault processing) | ~2ms (loopback + in-memory) |
| **Capacity** | ~500 RPS per sidecar | ~72k RPS per node |
| **Blast radius** | Pod crash = pod mất secret | Node crash = node mất cache, refetch từ Vault |
| **Startup** | Mỗi pod phải auth + fetch | Kallisto đã có sẵn, pod đọc ngay |

### Khi nào dùng

- Cluster có >50 pods cần secrets
- Latency requirement <5ms cho secret reads
- Muốn giảm Vault load mà không thêm Vault nodes
- Environments: staging, production (không phải dev — dev dùng K8s Secrets cho nhanh)

---

## Scenario 2: Edge / CDN PoP — "Vault ở xa, Kallisto ở gần"

### Vấn đề

Bạn có 20 CDN Points of Presence trên khắp thế giới. Mỗi PoP chạy:
- Nginx/Envoy reverse proxy
- Origin shield
- Edge functions (Cloudflare Workers style)

Các component này cần: TLS certs, API signing keys, origin auth tokens.
Vault cluster nằm ở `us-east-1`. Gọi Vault từ Singapore PoP = 200ms+ RTT.
Không chấp nhận được cho edge.

### Giải pháp: Kallisto tại mỗi PoP

```
          ┌─── Singapore PoP ───┐     ┌─── Frankfurt PoP ──┐
          │                     │     │                     │
          │  Nginx  EdgeFunc    │     │  Nginx  EdgeFunc    │
          │    │       │        │     │    │       │        │
          │    └───┬───┘        │     │    └───┬───┘        │
          │        │            │     │        │            │
          │  ┌─────▼──────┐    │     │  ┌─────▼──────┐    │
          │  │ Kallisto    │    │     │  │ Kallisto    │    │
          │  │ (local)     │    │     │  │ (local)     │    │
          │  │ ~2ms reads  │    │     │  │ ~2ms reads  │    │
          │  └──────┬──────┘    │     │  └──────┬──────┘    │
          └─────────┼───────────┘     └─────────┼───────────┘
                    │                           │
                    │      TLS (sync mỗi 30s)   │
                    └─────────┬─────────────────┘
                              │
                    ┌─────────▼─────────┐
                    │  Vault Cluster    │
                    │  (us-east-1)      │
                    └───────────────────┘
```

### Cách hoạt động

1. Kallisto ở mỗi PoP khởi động, auth với Vault, pull all secrets.
2. Secrets cached trong Sharded CuckooTable, encrypted-at-rest trên local disk.
3. Edge components đọc secrets từ local Kallisto: ~2ms thay vì 200ms.
4. Background sync mỗi 30s: Kallisto poll Vault cho changes (hoặc Vault push).
5. Nếu Vault unreachable: Kallisto vẫn serve từ local cache + RocksDB.
   Secrets không mất. Chỉ không nhận updates cho đến khi reconnect.

### Tại sao không dùng thứ khác

- **Replicate Vault cluster tới mỗi PoP?** — Quá nặng. Vault cần Raft quorum,
  tối thiểu 3 nodes per region. 20 PoPs × 3 = 60 Vault nodes. Chi phí insane.
- **HashiCorp Vault Agent caching?** — Vault Agent cache chỉ cache responses,
  không có sharded in-memory store, không có 72k RPS capacity.
- **Embed secrets vào container image?** — Security nightmare, rotation impossible.

---

## Scenario 3: Service Mesh mTLS Certificate Store

### Vấn đề

Service mesh (Istio/Linkerd) cần mTLS certificates cho mọi service-to-service
communication. Certificate rotation xảy ra thường xuyên (mỗi 1-24 giờ).
Mỗi sidecar proxy (Envoy) cần fetch cert + private key khi:

- Cold start
- Certificate rotation
- Reconnect after failure

Với 2000 Envoy sidecars rotate certs mỗi 1 giờ = ~33 cert fetches/second
liên tục. Cert signing thì vẫn phải qua Vault PKI engine, nhưng **đọc cert
đã signed** thì không cần.

### Giải pháp

```
Envoy Sidecar (mỗi pod)
    │
    │ SDS (Secret Discovery Service) API
    │
    ▼
Kallisto (DaemonSet, localhost)
    │
    │ Serve cert + key từ cache (~2ms)
    │
    │ Nếu cert sắp hết hạn:
    │   → Gọi Vault PKI issue new cert
    │   → Cache cert mới
    │   → Push SDS update tới Envoy
    │
    ▼
Vault PKI Engine (chỉ lúc signing)
```

Envoy hỗ trợ xDS/SDS API để nhận certificates. Kallisto có thể implement
một SDS server đơn giản, serve certs từ in-memory cache. Vault PKI chỉ
được gọi khi cần **sign cert mới**, không phải mỗi lần Envoy cần đọc cert.

### Kết quả

- **Vault PKI load giảm 99%**: từ 33 req/s xuống chỉ lúc rotation (vài req/phút).
- **Cert read latency**: ~2ms thay vì 10-50ms qua Vault.
- **Envoy cold start nhanh hơn**: không chờ Vault PKI, cert đã có sẵn ở Kallisto.

---

## Scenario 4: Database Proxy Credential Rotation

### Vấn đề

Bạn chạy PgBouncer / ProxySQL / Envoy database proxy trước PostgreSQL/MySQL.
Proxy cần database credentials để mở connection pool. Credentials rotate mỗi
8 giờ (best practice). Trong rotation window:

1. Vault generates new credential (dynamic secret)
2. Proxy cần fetch credential mới
3. Old connections drain, new connections dùng credential mới

Nếu proxy gọi Vault trực tiếp: mỗi lần mở connection mới = 1 Vault call.
Connection pool 200 connections × 10 proxy instances = 2000 Vault calls
trong 30 giây rotation window. Vault spike.

### Giải pháp

```
┌──────────────┐     ┌──────────────┐     ┌──────────────┐
│  App Pod 1   │     │  App Pod 2   │     │  App Pod N   │
└──────┬───────┘     └──────┬───────┘     └──────┬───────┘
       │                    │                    │
       └────────────────────┼────────────────────┘
                            │
                   ┌────────▼────────┐
                   │   PgBouncer     │
                   │   (DB Proxy)    │
                   └────────┬────────┘
                            │
               ┌────────────▼────────────┐
               │  GET /v1/secret/data/   │
               │    db/prod/postgres     │
               │                         │
               │  Kallisto (localhost)    │
               │  Response: ~2ms         │
               └────────────┬────────────┘
                            │
                 (background sync mỗi 5 phút)
                            │
                   ┌────────▼────────┐
                   │  Vault Database │
                   │  Secrets Engine │
                   └─────────────────┘
```

PgBouncer đọc credentials từ Kallisto (localhost, ~2ms). Kallisto background
sync với Vault Database engine. Khi credential rotate:

1. Kallisto nhận credential mới từ Vault
2. Update cache entry
3. PgBouncer polling Kallisto, thấy credential mới
4. Drain old connections, mở connections mới
5. Zero Vault spike. Smooth rotation.

---

## Scenario 5: CI/CD Pipeline Secret Injection

### Vấn đề

CI/CD pipeline (GitHub Actions, GitLab CI, Jenkins) cần secrets lúc build:
- Docker registry credentials
- Cloud provider keys (terraform apply)
- Code signing keys
- NPM/PyPI publish tokens

Bursty workload: 50 pipelines chạy đồng thời lúc merge → mỗi pipeline
cần 3-5 secrets → 150-250 Vault calls trong 10 giây. Vault xử lý được,
nhưng thêm latency vào mỗi pipeline stage.

Tệ hơn: nếu Vault cluster đang maintenance hoặc performance degraded →
toàn bộ CI/CD fleet bị stuck chờ secrets.

### Giải pháp

```
┌─── CI/CD Runner Node ─────────────────────┐
│                                            │
│  ┌─────────┐ ┌─────────┐ ┌─────────┐     │
│  │ Build 1 │ │ Build 2 │ │ Build N │     │
│  └────┬────┘ └────┬────┘ └────┬────┘     │
│       │           │           │           │
│       └───────────┼───────────┘           │
│                   │                       │
│          localhost:8200                    │
│                   │                       │
│       ┌───────────▼───────────┐           │
│       │  Kallisto             │           │
│       │  Pre-warmed secrets:  │           │
│       │  • registry creds    │           │
│       │  • cloud keys        │           │
│       │  • signing keys      │           │
│       └───────────┬───────────┘           │
│                   │                       │
└───────────────────┼───────────────────────┘
                    │ (sync)
                    ▼
              Vault Cluster
```

Kallisto chạy trên CI runner node, pre-warm secrets trước khi builds bắt đầu.
Builds đọc từ localhost: 2ms thay vì 50ms. Nếu Vault tạm unreachable, builds
vẫn chạy được với cached secrets (miễn TTL chưa hết).

---

## Scenario 6: Multi-Tenant SaaS — Per-Tenant Encryption Keys

### Vấn đề

Bạn build SaaS platform, mỗi tenant có encryption key riêng (tenant-level
encryption). 5000 tenants × mỗi request cần decrypt = 5000 key lookups.

Vault giữ 5000 encryption keys. Mỗi API request vào SaaS app:

1. Identify tenant
2. Fetch tenant's encryption key từ Vault
3. Decrypt request data
4. Process
5. Encrypt response data
6. Return

Step 2 thêm 10-50ms vào mỗi request. Ở 10k req/s, đó là 10k Vault
calls/second. Vault sẽ chết.

### Giải pháp

```
                         ┌──────────────┐
                         │ API Gateway  │
                         └──────┬───────┘
                                │
                    ┌───────────▼───────────┐
                    │  SaaS App Server      │
                    │                       │
                    │  1. Identify tenant   │
                    │  2. GET tenant key    │──→ Kallisto (localhost)
                    │     from Kallisto     │    ~2ms, 72k RPS capacity
                    │  3. Decrypt/process   │
                    │  4. Return            │
                    └───────────┬───────────┘
                                │
                    (background: key rotation sync)
                                │
                       ┌────────▼────────┐
                       │  Vault Transit  │
                       │  (key store)    │
                       └─────────────────┘
```

5000 tenant keys cached trong Kallisto. Mỗi API request đọc key từ
localhost: 2ms. Vault chỉ được gọi khi: (a) key rotation, (b) new
tenant onboarding, (c) cache miss cho tenant mới. Vault load giảm
từ 10k req/s xuống <10 req/s.

---

## Scenario 7: API Gateway Credential Store

### Vấn đề

API Gateway (Kong, Envoy, custom) cần validate incoming requests:
- API key lookup: request có API key → check key exists + permissions
- OAuth client secret validation
- Rate limit key → tier lookup

Với 50k req/s qua gateway, mỗi request cần 1-2 secret lookups.
Dùng Redis? Redis không encrypt-at-rest, API keys nằm trần trong RAM dump.
Dùng Vault? 50k req/s, Vault sẽ khóc.

### Giải pháp

Kallisto ngồi cạnh gateway, serve API key lookups ở wire speed.
API keys encrypted-at-rest trên disk. ACL đảm bảo chỉ gateway process
đọc được. 72k RPS capacity đủ cho gateway ở moderate load.

```
Client → API Gateway → Kallisto (api key validate, ~2ms)
                      → Backend service
```

---

## Scenario 8: Disaster Recovery — "Vault chết, hệ thống vẫn chạy"

### Vấn đề thực tế

Vault cluster outage. Nguyên nhân: bad upgrade, storage corruption,
network partition, cloud provider incident. Thời gian phục hồi: 30
phút đến 2 giờ.

Trong thời gian đó, **mọi service cần secrets đều chết.** Không đọc
được DB password → không connect DB → 503 toàn bộ.

### Giải pháp

Kallisto đã có secrets trong local RocksDB (encrypted). Khi Vault
unreachable:

1. Kallisto phát hiện Vault down (health check fail)
2. Chuyển sang **degraded mode**: serve từ local cache, từ chối writes
3. Log warning: "Operating in degraded mode, secrets may be stale"
4. Khi Vault recovery: Kallisto tự reconnect, sync lại, resume normal

**Secrets không mất. Services không chết. Bạn có thời gian fix Vault.**

Đây giống cách DNS caching hoạt động: DNS root server chết thì
resolvers vẫn serve từ cache cho đến khi TTL hết.

---

## Tổng kết: Pattern chung

Nhìn lại 8 scenarios, pattern chung là:

> **Kallisto = Caching proxy chuyên biệt cho secrets, đứng giữa
> consumers (apps, proxies, pipelines) và source of truth (Vault).**

Giống như:
- **Varnish** đứng trước web server → cache HTTP responses
- **PgBouncer** đứng trước PostgreSQL → cache DB connections  
- **Local DNS resolver** đứng trước root servers → cache DNS records

**Kallisto đứng trước Vault → cache secrets.**

Mỗi scenario ở trên đều tuân theo cùng một nguyên tắc:

1. **Source of truth vẫn là Vault** — không thay đổi
2. **Kallisto là read-heavy cache layer** — giảm load cho Vault
3. **Localhost access** — loại bỏ network latency
4. **Encrypt-at-rest** — secrets an toàn ngay cả trên local disk
5. **Graceful degradation** — Vault down ≠ system down

Không có sản phẩm nào trên thị trường hiện tại làm chính xác điều này.
Đó là lý do Kallisto đáng để code tiếp.
