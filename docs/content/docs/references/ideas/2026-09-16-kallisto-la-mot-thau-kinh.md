---
title: "Kallisto là một thấu kính"
date: 2026-09-16
status: "exploration"
---

# Kallisto là một thấu kính

Trạng thái: thăm dò. Chưa có gì được chốt ở đây. Tài liệu này ghi lại một buổi
suy nghĩ lại về hình dạng của Kallisto, kèm những chỗ từng hướng đi bị gãy.

Câu hỏi mở đầu: Kallisto khoảng 5.700 dòng code. Có nhất thiết phải dựng một
webserver đồ sộ để nó nhanh không? Nó có đạt tới sự thanh lịch của WireGuard,
SQLite, DuckDB — những thứ đơn giản đến mức ngu ngốc nhưng lại là đỉnh của nghề
— được không?

## 1. Đo trước đã

Số thật tại thời điểm viết:

- 5.678 dòng Rust không tính test. 7.759 nếu tính cả test, fuzz, bench.
- `src/server/http_handler.rs` là 746 dòng, và phần lớn là logic KV-v2
  (merge patch, subkeys, version list), không phải HTTP.
- Cây phụ thuộc runtime của `kallisto-server`: khoảng 98 crate. Trong đó chừng
  30 crate là tầng HTTP/async.

Kết luận: chúng ta không hề đang viết một webserver đồ sộ. Cách dùng `axum`
hiện tại đã mỏng tới mức gần như chỉ là router — handler nhận `Bytes` thô, tự
cắt URI, tự parse query param. Cảm giác nặng đến từ cây phụ thuộc, không đến từ
code đã viết.

## 2. Sự thanh lịch đến từ đâu

Nhận định sai cần sửa: WireGuard, SQLite và DuckDB không thanh lịch vì ít dòng
code. Chúng thanh lịch vì **đóng không gian thiết kế lại**.

WireGuard đơn giản vì nó từ chối cipher agility: đúng một bộ, không đàm phán.
SQLite đơn giản vì nó từ chối làm server.

Và cả ba đều không tự viết phần nguy hiểm nhất. SQLite không làm networking.
WireGuard không làm key distribution.

Hệ quả cho Kallisto: tự viết HTTP/1.1 parser cho một dịch vụ phân phối secret
là đi ngược lại đúng bài học đó. Request smuggling và chunked encoding là bãi
mìn CVE; `httparse`/`hyper` đã bị fuzz nhiều năm. Bỏ chúng nghĩa là trở thành
maintainer của 1.500 dòng nguy hiểm nhất dự án.

Nước đi hợp lệ nếu muốn gọn hơn: bỏ `axum`, dùng `hyper` trực tiếp. Có đúng 7
route cố định, và URI vốn đã tự parse. Giữ được parser đã kiểm chứng, bỏ được
tầng tower/matchit/extractor, chừng 10-12 crate. Nhưng phải đo trước bằng
`benchmarks/server` — nếu RocksDB và JSON đang chiếm ưu thế thì đây chỉ là
thẩm mỹ.

## 3. DNS là từ vựng, không phải đích đến

Trực giác ban đầu: Kallisto giống một cụm DNS hơn là một cụm cache. Đúng.

Nhưng DNS không có một giao thức. Nó có hai quan hệ: stub resolver với local
resolver (đơn giản, trên loopback), và resolver với authoritative (federated,
có delegation, TTL, NOTIFY, transfer). Phần thanh lịch nằm ở vế thứ hai.

Kallisto cũng có hai vế đó, và **vế thứ hai hiện đang trống**: chưa có cache
TTL, chưa có logic làm mới từ upstream, chưa có invalidation. Trường `ttl` trong
`src/engine/traits.rs` là `delete_version_after` của KV-v2, không phải cache TTL.

Nên: giữ KV-v2 ở loopback (nó là toàn bộ đòn bẩy adoption — đổi một dòng
`VAULT_ADDR`), và đặt cách nghĩ DNS vào chỗ đang trống.

DNS cho sẵn những thứ README đã hứa mà chưa làm:

- NOTIFY — đẩy invalidation xuống node.
- IXFR/AXFR — làm ấm cache trước rollout, chỉ chuyển phần đã đổi.
- SOA serial + refresh/retry/expire — mô hình cache coherence, 40 năm vận hành.
- Negative caching — nhớ cả những thứ *không tồn tại*. Đây là lỗ hổng self-DDoS
  thật: app gõ sai đường dẫn, retry 100 lần/giây, Kallisto hỏi Vault 100
  lần/giây. ADR-0011 chưa có mục này.
- Delegation qua NS — federation. Thứ Vault Namespaces làm dở và chỉ có ở bản
  Enterprise.

Cảnh báo: không dùng RFC 1035 nguyên bản trên cổng 53. Ngoài cái bẫy
decompression-loop của name compression pointer, vấn đề lớn hơn là mọi middlebox
trên đường đi (systemd-resolved, resolver của ISP, DoH trong browser) sẽ vô tư
chạm vào traffic. Một secret đi lạc vào resolver thật là lỗi không rút lại được.
Mượn ngữ nghĩa, tự làm wire format, cổng riêng.

Ghi nhận: ADR-0011 thật ra **đã là** mô hình DNS rồi, chỉ chưa gọi tên.
`X-Kallisto-Age` chính là TTL. Fail-open khi upstream chết chính là cách resolver
phục vụ bản cũ. Chỉ nạp cache từ upstream chính là chống cache poisoning.

## 4. Quan hệ với ADR-0009 (ReDB + Raft)

Không xung đột. Hai thứ nằm ở hai chế độ khác nhau:

- Sovereign mode — Kallisto là chủ dữ liệu, cần chắc chắn. Chỗ của Raft + ReDB.
- Proxy mode — Kallisto chỉ là bản sao. Chỗ của cách nghĩ DNS. Không cần Raft.

Một hệ quả phụ đáng giá: ADR-0009 bỏ RocksDB ở Sovereign, ADR-0011 bỏ RocksDB ở
Proxy. Vậy RocksDB biến mất hoàn toàn, kéo theo cả dây C++: `librocksdb-sys`,
`libz-sys`, `lz4-sys`, `bzip2-sys`, `zstd-sys`, `bindgen`, `clang-sys`. Khoảng
8-10 crate, cộng với việc không còn cần toolchain C++ để build.

Đây là ví dụ cụ thể nhất cho luận điểm ở mục 2: sự thanh lịch đạt được bằng một
quyết định xoá, không phải một quyết định viết.

Việc kèm theo: `rkyv 0.7` đang là mục đỏ duy nhất của cargo-deny, và lý do chưa
sửa được là vì nâng cấp đồng nghĩa với migrate dữ liệu. Sắp migrate sang ReDB
rồi — đó là lúc rẻ nhất để xử luôn, làm một lần thay vì hai.

## 5. Lăng kính DuckDB

Trò ảo thuật thật sự của DuckDB không phải tốc độ. Là: nó không bắt bạn di
chuyển dữ liệu của mình. `SELECT * FROM 'file.parquet'`. Không import, không
migrate. Nó đi tới chỗ dữ liệu.

Soi Kallisto qua đó:

**Thư viện là sản phẩm, server là cái vỏ.** `cmd/kallisto-server/src/main.rs`
có 211 dòng và gần hết là parse tham số. Nó vốn đã là vỏ mỏng bọc `KallistoCore`.
Nói thẳng ra: `libkallisto` là sản phẩm, `kallisto-server` là bản dịch cho ai
không link được thư viện. Cách nghĩ này gỡ nút thắt về KV-v2 — ta giữ nó vì nó
là **miếng chuyển đổi**, không phải vì nó là kiến trúc.

**Không có file config.** Hiện `DEFAULT_WORKERS: usize = 2` là số cứng; máy 64
nhân cũng 2. Kiểu DuckDB: đọc số nhân, đọc giới hạn cgroup, tự chia. ADR-0003
("Configuration format") nên bị hỏi lại — câu trả lời kiểu DuckDB cho "dùng định
dạng config nào" là "không có file config".

**Đọc secret ở nơi nó đang nằm.** Mount thẳng: `secret/` tới Vault, `env/` tới
một file `.env`, `docker/` tới Docker secrets, `k8s/` tới Secret đã mount. Tất
cả hiện ra sau một bộ mặt KV-v2. `EngineRegistry` đã làm sẵn cơ chế này.

Phân biệt quan trọng: cắm nhiều **kho ghi** (chỗ Kallisto tự cất dữ liệu của
mình) là cái bẫy — đó là chuyện nội bộ, agility ở đó chỉ là chưa nghĩ xong. Cắm
nhiều **nguồn đọc** (chỗ dữ liệu người khác đang nằm) là nước đi DuckDB — đó là
chuyện đối ngoại, và nó chính là giá trị.

**Một ý tưởng duy nhất, áp dụng ở mọi nơi.** DuckDB có vector 2048 dòng.
Kallisto nên có **bản ghi đã khoá**: một secret, khoá sẵn cho đúng người nhận,
và là cùng một vật thể trong RAM, trên đĩa, trong log, trên đường truyền, trong
cache. Chỉ mở ở giây phút giao cho app. Nếu chốt điều này thì mục "log phải được
mã hoá" của ADR-0009 tự giải quyết, cache ở Proxy mode an toàn mặc định, và con
số "bao nhiêu plaintext đang nằm trên hạm đội" đo được và gần bằng 0.

**Những chỗ nên ngu.** Không ngôn ngữ policy. Không UI (ADR-0004 cứ treo).
Không vườn thú auth method — chọn một. Khởi động lại thì cache trống và thế là
xong, đừng xây cơ chế giữ cache qua restart. Đừng khôn ngoan về eviction —
CLOCK đủ rồi, một node thật sự chỉ cần vài chục tới vài trăm secret.

**Một chỗ đang quá khôn.** ADR-0009 ghi Sovereign ghi "vài lần một phút".
ADR-0008 định dùng hàng đợi Vyukov gom lô cho Raft, và chính nó ghi ở mục
conflict rằng với tải đó mỗi lô chỉ có 1 entry còn client ăn nguyên 5ms chờ vô
ích. Câu trả lời đã nằm sẵn trong ADR mà chưa rút ra: với vài ghi mỗi phút,
đừng gom lô gì cả. Một thread ghi, `fsync`, xong. Để hàng đợi lock-free phục vụ
việc nạp cache — chỗ đó mới đông.

## 6. Mượn nền tảng thay vì tự xây

Docker secret là ghi một chiều. `docker secret inspect` chỉ trả metadata, không
trả giá trị. Cách duy nhất lấy được là **trở thành một task được Swarm giao**,
và nó nằm ở `/run/secrets/<tên>` trên tmpfs.

Ràng buộc đó hoá ra tốt: trong compose file phải viết ra bằng chữ đúng những
secret Kallisto được cầm. Đó chính là yêu cầu số 3 của ADR-0011 (root of trust
phải hẹp), do Swarm ép bằng khai báo, không cần code.

Sửa một hiểu nhầm: Swarm **không** sync secret toàn cụm. Nó giữ trong Raft log
của manager, mã hoá khi lưu, và chỉ giao xuống node đang chạy task cần nó, qua
mTLS, vào tmpfs của đúng task đó, xoá khi task dừng. Đó là giao theo nhu cầu.

Tức là Docker Swarm đã là một control plane: Raft ở manager, đẩy theo nhu cầu,
không chạm đĩa. Gần đúng cái ADR-0006/0009/0001 đang mô tả. Câu hỏi mở: trong
môi trường Swarm, Kallisto có cần Sovereign mode không, hay chỉ nên là bộ mặt
phục vụ đặt lên trên thứ đã có?

Rủi ro phải ghi rõ: Swarm cẩn thận chia nhỏ, Kallisto gom hết lại rồi mở một
cổng. **Kallisto trở thành máy khuếch đại secret**, phá bằng sạch công chia ngăn
của Swarm. README hiện ghi rõ cổng dữ liệu chưa có xác thực.

Phần dễ nhất mà có khi giá trị nhất: Compose thường (không Swarm) thì secret chỉ
là một file được mount. App ở laptop đọc file, lên prod đọc Vault — hai đường
code, và đường ở laptop luôn ít được test hơn. Kallisto mount `/run/secrets/` ra
KV-v2 thì app chỉ còn một đường duy nhất. Đây là một `SecretEngine` chỉ đọc,
cắm vào `EngineRegistry`, chừng 80 dòng.

Chi tiết dễ vấp: secret của Swarm là bất biến. Xoay khoá = tạo mới + cập nhật
service = task bị tạo lại = Kallisto restart. Nghe phiền, nhưng đó đúng là câu
trả lời "ngu mà thanh lịch": cache trống, nạp lại vài mili giây, xong. Không
phải viết dòng nào cho xoay khoá.

## 7. Blob đã khoá trên object store

Hướng đi xa nhất, và có thể là đích thật sự.

Kallisto nhận một khoá do nền tảng mount vào. Đọc một blob đã mã hoá từ S3 (hoặc
bất cứ đâu). Giải mã trong RAM. Phục vụ KV-v2. Không database, không Raft,
không đĩa.

Đây chính là `SELECT * FROM 's3://bucket/file.parquet'` của DuckDB, bằng secret.

Nó xoá: ReDB, `openraft`, snapshot, cắt log, mã hoá log — tức là cả ADR-0008 và
ADR-0009. Quan trọng hơn số dòng: nó xoá việc trực đêm. Raft nghĩa là ta sở hữu
bài toán mất quorum, split-brain, đầy đĩa trên leader. S3 nghĩa là bài toán đó
của Amazon.

Mô hình không mới — SOPS và sealed-secrets đã làm nhiều năm. Thứ chúng không có
và Kallisto sẽ có là **cái mặt tiền**: một endpoint KV-v2 sống, chứ không phải
một file đã giải mã lúc deploy. Chỗ trống đó chưa ai lấp.

Bốn chỗ phải cẩn thận:

1. "Không thể chết" sai một chỗ. Kallisto đang chạy mà S3 sập thì không sao, dữ
   liệu vẫn trong RAM. Nhưng restart *trong lúc* S3 sập thì có số không. K8s
   evict pod bất cứ lúc nào nó thích, và hai sự kiện này sẽ gặp nhau. Chữa mà
   vẫn ngu: giữ một bản sao blob trên đĩa, **vẫn đang khoá**. Không vi phạm
   ADR-0001 vì thứ ghi xuống đĩa là bản mã.
2. Một khoá mở được tất cả. Node bị chiếm là mất sạch, chọi thẳng với yêu cầu số
   3 của ADR-0011. Chữa: nhiều blob hơn, mỗi ranh giới tin cậy một blob, mỗi
   blob một khoá. `EngineRegistry` lo được, không thêm gì.
3. Đường ghi. Trung thực nhất là trả 405, chỉ đọc — ghi là việc của pipeline.
   Nếu cần ghi: S3 hỗ trợ ghi có điều kiện theo ETag, ánh xạ thẳng vào tham số
   `cas` của KV-v2. Cùng một ý niệm, không cần lớp dịch.
4. Biết blob đã đổi bằng cách nào: `GET` kèm ETag, chưa đổi thì `304`, gần như
   không tốn gì. ETag chính là SOA serial của mục 3. Vòng tròn khép lại.

Cảnh báo cụ thể: **đừng kéo `aws-sdk-s3` vào**, nó là cây phụ thuộc rất lớn và
sẽ trả lại hết phần vừa tiết kiệm được từ việc bỏ RocksDB. Thứ cần chỉ là "lấy
một mớ byte từ một URL" — SigV4 tự ký, hoặc presigned URL thì chỉ còn là một
`GET` thường. Làm vậy cũng không bị trói vào S3: MinIO, R2, GCS, một file HTTPS
tĩnh, một repo git đều chạy.

Câu hỏi mở quan trọng nhất của tài liệu này: **Sovereign mode để làm gì nữa?**
Nó sinh ra cho trường hợp "không có Vault ở trên". Blob-trên-S3 cũng phục vụ
đúng trường hợp đó mà không cần Raft. Sovereign hơn ở chỗ ghi nhanh hơn — nhưng
chính ADR-0009 ghi là vài ghi mỗi phút — và ghi được khi object store chết, đổi
lại là cả một cụm Raft để trực. Cần hỏi lại trước khi viết dòng `openraft` đầu
tiên.

## 8. Luận đề rút gọn

**Kallisto không giữ secret. Nó chỉ cho secret một cái mặt để hỏi.**

Mọi thứ khác — cuckoo table, Raft, ReDB, gossip — là chi tiết thi công.

Hệ quả cho README: câu mở đầu hiện tại là "high-performance cache for secrets",
tức là mô tả cách nó chạy. Câu đúng là mô tả điều nó đổi được cho người dùng:
**app của bạn không cần `.env` nữa, nó chỉ cần hỏi.**

Và phải nói thẳng ở dòng đầu, không giấu trong mục lưu ý: nếu Kallisto không giữ
gì cả thì Kallisto không phải hệ quản lý secret, nó là một thấu kính. Ai đó khác
vẫn phải giữ secret thật. Điều đó hoàn toàn ổn — WireGuard cũng không làm key
distribution — nhưng phải được tuyên bố.

## 9. Danh tính workload: đường đã loại

Đây là chỗ duy nhất không ngu được, và là chỗ một hướng đi đã bị loại bỏ.

**Đã thử và loại: Unix domain socket.** Ý tưởng là mỗi consumer một socket
riêng, đường dẫn socket chính là danh tính, và `SO_PEERCRED` cho biết PID/UID.
Nó gãy vì:

- `SO_PEERCRED` trả về UID và PID. Trong container mọi thứ chạy dưới `root` hoặc
  đúng một user `app`. Nhiều instance cùng UID thì PEERCRED không phân biệt được
  gì. Nó trả lời một câu hỏi không ai hỏi.
- PID thì đua: bị tái sử dụng, và map PID sang workload phải đọc
  `/proc/PID/cgroup`, vừa đua vừa phụ thuộc runtime.
- Bind-mount thẳng *file* socket rồi Kallisto restart thì inode cũ chết, container
  vẫn trỏ vào cái xác, không có lỗi rõ ràng.
- Trên k8s, chia socket giữa các pod buộc phải dùng hostPath, thường bị chính
  sách chặn.

`components/kallisto_cluster/src/admin_uds.rs` đang là file 0 byte. Xoá đúng.

**Đường còn lại: đừng có cửa để khoá.** Kallisto không đứng ngoài ranh giới rồi
canh cửa — nó đứng **bên trong** ranh giới đã có sẵn. Chạy như sidecar, chung
network namespace với pod hoặc task, nghe `127.0.0.1:8200` bên trong namespace
đó. Ai vào được? Đúng những container trong pod đó. Ai chặn? Kernel, bằng đúng
cái netns mà nền tảng vốn đã dùng để cô lập mọi thứ khác.

Không có quyền file để đánh nhau, không có UID để phân biệt, không có token để
phát. Kallisto không xác thực gì cả, vì không còn ai lạ để xác thực. Vault Agent
cũng làm đúng vậy — cache của nó nghe loopback và bất kỳ tiến trình nào trong
cùng ranh giới đều dùng được.

**Khi ranh giới pod không đủ mịn:** nền tảng phát danh tính, Kallisto chỉ kiểm
tra. Projected ServiceAccount token của k8s, hoặc SVID của SPIFFE — kubelet tự
mount, tự xoay, app gửi lên, Kallisto verify chữ ký. Không phát, không cất,
không xoay. ADR-0011 đã ghi JWKS, đó chính là đường này, và nó *nên* đắt vì chỉ
lôi ra khi thật sự cần.

**Cái giá:** sidecar nghĩa là một Kallisto mỗi pod, không phải mỗi node. Trước
đây sẽ đau vì mỗi instance là một kết nối nữa đấm lên Vault. Nhưng với hướng
blob-trên-S3 ở mục 7, nỗi lo đó biến mất — S3 không quan tâm có bao nhiêu thằng
đọc. Hai ý tưởng tự đỡ cho nhau: bỏ Vault khỏi đường nóng thì mô hình sidecar
trở nên trả được, và mô hình sidecar thì xoá sạch bài toán phân quyền.

## 10. Sức ép lên các ADR hiện có

Tài liệu này không sửa ADR nào. Nhưng nó đặt câu hỏi cho:

- ADR-0003 (Configuration format) — có nên không có file config?
- ADR-0006 / ADR-0008 / ADR-0009 (Raft, group commit, ReDB) — Sovereign mode có
  còn cần thiết nếu có blob-trên-S3? Và nếu còn, group commit có vô nghĩa ở tải
  vài ghi/phút không?
- ADR-0010 (ranh giới Sovereign) — biến thể "có Vault ở trên" và hướng blob có
  gộp làm một được không?
- ADR-0011 (Proxy mode) — bổ sung negative caching; và chốt mô hình sidecar +
  netns thay cho xác thực trên cổng dữ liệu.

## 11. Việc nhỏ có thể làm ngay

- `SecretEngine` chỉ đọc cho `/run/secrets/` và file `.env`. Chừng 80 dòng, cắm
  vào `EngineRegistry`.
- `DEFAULT_WORKERS` lấy từ số nhân và giới hạn cgroup thay vì hằng số 2.
- Negative caching cho Proxy mode, thời hạn ngắn hơn TTL thường.
- Xoá bốn component stub `pub fn hello()` và hai thư mục rỗng `src/net/`,
  `src/thread_local/`. Scaffolding rỗng hứa hẹn một kiến trúc chưa tồn tại.
