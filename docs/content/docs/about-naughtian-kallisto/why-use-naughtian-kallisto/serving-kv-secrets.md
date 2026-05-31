---
title: "Serving High-Rate-Access KV Secrets"
weight: 2
---

Kallisto sử dụng cơ chế **Write-Behind (Ghi đệm hoãn lại)** để ngắt kết nối trực tiếp giữa luồng xử lý chính của HTTP Epoll Worker và luồng ghi ổ đĩa chậm của RocksDB.

### Kết quả đo lường (Benchmarks)
*   **RPS tối đa**: Vượt mốc **91,000+ RPS** cho tác vụ ghi (PUT).
*   **Độ trễ Đọc (GET p99)**: Chỉ **2.63ms**.
*   **Độ trễ Ghi (PUT p99)**: Ổn định ở mức **9.43ms** dưới áp lực tải cực đại.
