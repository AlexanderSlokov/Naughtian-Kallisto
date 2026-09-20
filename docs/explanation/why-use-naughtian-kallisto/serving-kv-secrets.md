# Phục vụ KV secret với tần suất đọc cao

Trang này là bản chuẩn cho **đường đọc**: vì sao nó được coi là đường nóng, nó được xây thế nào,
và nó đo được bao nhiêu. Kallisto là gì thì xem
[what-is-kallisto.md](../what-is-kallisto.md); hình dạng từng tầng xem
[architecture.md](../architecture.md).

## Vấn đề

Khi một ứng dụng liên tục đọc lại vài secret, một Vault trung tâm thêm một vòng round trip mạng
vào đường request, và trở thành điểm chết: Vault không với tới được thì ứng dụng ngừng chạy.
Kallisto giữ giá trị hiện tại ngay tại localhost, làm mới theo chu kỳ, và vẫn phục vụ từ bản sao
đã mã hoá trên đĩa nếu bucket sập.

## Vì sao đường đọc là đường nóng

ADR-0015 ban đầu cho rằng một luồng là quá đủ, vì phần lớn ứng dụng đọc secret một lần lúc khởi
động rồi giữ trong bộ nhớ. ADR-0016 đảo lại quyết định đó, vì tiền đề ấy sai với một lớp ứng dụng
có thật: **lấy secret ngay trước mỗi lần dùng, dùng xong huỷ ngay, không cache gì cả.**

Pattern đó cực đoan nhưng hợp lý — nó thu hẹp khoảng thời gian secret nằm trong bộ nhớ ứng dụng
xuống mức nhỏ nhất, và đúng với tinh thần bảo mật của chính dự án. Với pattern đó, mỗi lần dùng
secret là một request, nên rps không hề nhỏ và đường đọc *là* đường nóng.

## Đường đọc được xây thế nào

Một Tokio runtime `current_thread` cho mỗi worker, ghim vào core bằng `core_affinity`, nhiều
worker chung một cổng qua `SO_REUSEPORT`. Không work-stealing.

Router được dựng một lần cho mỗi worker, **trên chính thread của worker đó**. Nghĩa là rate
limiter, producer của access log và các counter đều thuộc về một core và không bao giờ bị chia sẻ
— đường đọc không chạm vào một cache line dùng chung nào. Rate limit vì thế tính theo *từng
worker*, và tên trường cấu hình nói rõ điều đó.

Cổng ra được **hàn chết** (ADR-0016 QĐ-4): không có trait `SecretEngine`, không registry, không
`Arc<dyn>` trên đường đọc. Handler nạp snapshot hiện tại rồi đọc một `HashMap`. Trừu tượng từng
nằm ở đây thu phí một lần dynamic dispatch mỗi request để mua một khả năng thay thế không ai yêu
cầu.

Secret được seal **riêng lẻ** trong RAM dưới một khoá riêng của từng snapshot, và chỉ được mở vào
một buffer `thread_local` trong đúng thời gian một response. Thân response được dựng *bên trong*
callback đó, nên cleartext tồn tại đúng bằng thời gian copy nó vào body, rồi buffer bị wipe.

Quá tải được trả lời theo cách Vault trả lời: 429 kèm `Retry-After`.

## Số đo

Laptop 8 core, 4 worker, wrk2 trên cùng máy:

| Phép đo | Kết quả |
|---|---|
| Thông lượng bão hoà | ~99k req/s |
| p50 / p99 ở 30k req/s | 1.3 ms / 3.5 ms |
| Barrier mã hoá trong RAM | 350–430 ns mỗi request |
| Access log | ~6% đuôi p99 |

Barrier tốn dưới 1% ngân sách thời gian của một request: ở trần ~99k req/s trên 4 worker, mỗi
worker có khoảng 40 µs cho một request. Nó chìm dưới nhiễu ở thông lượng và chỉ ló ra ở đuôi p99.

Cả hai con số chi phí đó được đo bằng cách **xen kẽ hai binary trong cùng một vòng lặp**, sau khi
đo riêng từng bản cho ra kết quả vô nghĩa. Phương pháp nằm ở
[plans/duck/plan.md](../../references/plans/duck/plan.md).

Chạy lại: `make bench-laptop` (30k req/s, kỳ vọng p50 ~1.3 ms) hoặc `make bench-duck`.

## Không có đường ghi

Kallisto không ghi. Mọi route ghi trả 403, kể cả `subkeys`, `delete`, `undelete`, `destroy` — chúng
được khai báo tường minh để trả 403 chứ không phải 404, vì 404 đọc ra thành "chưa triển khai" còn
403 nói rằng cửa có thật và đang đóng.

Điều này đáng nói vì [kho benchmark](../../references/benchmarks/) còn giữ các số đo **PUT** từ
thời trước: 91k RPS ghi, p99 9.43 ms. Chúng đo một đường ghi đã bị ADR-0015 xoá — giữ lại làm hồ
sơ lịch sử, không phải mô tả hệ thống hiện tại.
