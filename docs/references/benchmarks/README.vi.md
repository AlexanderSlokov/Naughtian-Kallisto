# Báo cáo hiệu năng (Benchmarks)

Kho lưu các kết quả benchmark của Kallisto qua từng giai đoạn, xếp theo thời gian. Phần lớn ở đây
là **hồ sơ lịch sử, không phải mô tả hiện trạng**: các báo cáo bên dưới có trước ADR-0015, và những
cơ chế chúng đo — SipHash với sharded cuckoo table, B-tree index đường dẫn, write-behind giảm tải
I/O cho RocksDB — đều đã bị xoá cùng storage engine. Báo cáo nào ghi `Admin Port: 8202` hay
`BATCH mode` là đang đo một hệ thống không còn tồn tại.

Vẫn giữ lại, vì một quyết định có dẫn số đo thì phải để số đo ở chỗ kiểm được. ADR-0016 dựa trên
các con số trong kho này, và `verification-status.md` phân biệt cái gì đã chứng minh với cái gì chỉ
là tin vậy.

**Số đo của đường đọc hiện tại nằm ở
[serving-kv-secrets.md](../../explanation/why-use-naughtian-kallisto/serving-kv-secrets.md).** Đó
là trang chuẩn, và phương pháp đo hai con số chi phí ở
[plans/duck/plan.md](../plans/duck/plan.md).

Chạy lại tại máy: `make bench-laptop` hoặc `make bench-duck`; `cargo bench` chạy bộ Criterion
in-process cho barrier.
