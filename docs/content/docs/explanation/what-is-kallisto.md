---
title: "Naughtian Kallisto là gì?"
weight: 1
---

# Naughtian Kallisto là gì?

**Naughtian Kallisto** là một giải pháp quản lý bí mật vận hành mã nguồn mở, được xây dựng để cung cấp khả năng quản lý và phân phối thông tin mật với tốc độ cực lớn (đạt hàng trăm ngàn lượt truy xuất mỗi giây ở mức microsecond).

### Đặc trưng chính

*   **Tích hợp Vault Transit**: Ủy quyền quản lý Master Key hoàn toàn cho các Root of Trust ngoại vi thông qua cơ chế mã hóa phong bì (Envelope Encryption).
*   **Bộ nhớ đệm Lock-free**: Sharded Cuckoo Table hỗ trợ đọc ghi song song chịu tải lớn mà không gây tắc nghẽn khóa luồng (mutex lock contention).
