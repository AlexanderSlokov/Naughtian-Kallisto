---
title: "Bảo mật tuyệt đối"
weight: 2
---

# Bảo mật tuyệt đối (Security) 🛡️

Kiến trúc bảo mật lớp ghép (Core-Armor pattern) của Kallisto tách biệt tuyệt đối ranh giới xử lý:

### Các trụ cột bảo mật
1.  **Vault Transit làm Root of Trust**: Master Key của bạn không bao giờ rời khỏi máy chủ Vault. Kallisto chỉ nhận KEK và lưu tạm trong RAM.
2.  **Rust Memory Safety**: Đảm bảo bộ nhớ KEK không bị tràn hoặc truy cập bất hợp pháp.
3.  **RAM Zeroization**: Sử dụng thư viện `zeroize` để ghi đè `0` lên toàn bộ vùng RAM lưu trữ khóa ngay khi tắt hoặc giải phóng đối tượng khóa, tránh triệt để tấn công đọc trộm RAM (Cold Boot Attack).
