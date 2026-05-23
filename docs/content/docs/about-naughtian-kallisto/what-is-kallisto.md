---
title: "Naughtian Kallisto là gì?"
weight: 1
---

# Naughtian Kallisto là gì?

**Naughtian Kallisto** là một giải pháp quản lý thông tin mật (Secret Management) mã nguồn mở, được xây dựng để cung cấp khả năng bảo mật thông tin mật tối đa với tốc độ cực lớn (đạt hàng trăm ngàn lượt truy xuất mỗi giây ở mức microsecond).

### Đặc trưng chính
*   **Mô hình Hybrid C++/Rust**: Tận dụng hiệu năng I/O tối đa của C++ kết hợp với độ an toàn bộ nhớ tuyệt đối của Rust.
*   **Tích hợp Vault Transit**: Ủy quyền Master Key hoàn toàn cho Vault (Root of Trust) thông qua cơ chế mã hóa phong bì (Envelope Encryption).
*   **Bộ nhớ đệm Lock-free**: Sharded Cuckoo Table hỗ trợ đọc ghi song song chịu tải lớn mà không gây tắc nghẽn khóa luồng (mutex lock contention).
