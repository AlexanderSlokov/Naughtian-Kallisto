---
title: "Benchmarks & Báo cáo hiệu năng"
linkTitle: "Benchmarks"
weight: 10
---

# 🚀 Báo cáo hiệu năng (Benchmarks)

Chào mừng bạn đến với chuyên mục lưu trữ các kết quả benchmark của Kallisto qua từng giai đoạn phát triển. 

Kallisto sinh ra để trở thành một "cỗ máy tốc độ" (High-Performance Secret Engine) nên việc theo dõi và tối ưu hóa hiệu năng là ưu tiên hàng đầu. Tại đây, chúng tôi lưu giữ các số liệu chứng minh tốc độ xử lý của các cơ chế cốt lõi như:

*   **SipHash & Cuckoo Table:** Khả năng tra cứu $O(1)$ tuyệt đối, chống lại các cuộc tấn công Hash Flooding.
*   **B-Tree Indexing:** Hệ thống gác cổng tối ưu, xác thực đường dẫn ở tốc độ cực cao.
*   **Sharded Concurrency:** Khả năng xử lý đa luồng (Multi-threading) không tắc nghẽn thông qua kiến trúc phân rã ổ khóa.
*   **Write-Behind (Eventual Consistency):** Hiệu năng của hàng đợi không khóa (Lock-free queue) khi giảm tải I/O cho RocksDB.

Bên dưới là các phiên bản báo cáo benchmark chi tiết. Các báo cáo này ghi nhận từ tốc độ thô (Core Engine) cho tới khả năng chịu tải trên giao thức HTTP (Server Load).
