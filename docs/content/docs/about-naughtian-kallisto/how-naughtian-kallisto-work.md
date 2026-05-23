---
title: "Kallisto hoạt động như thế nào?"
weight: 3
---

# Kallisto hoạt động như thế nào? ⚙️

Quy trình xử lý một yêu cầu lưu trữ thông tin mật (PUT/GET Secret Entry) diễn ra qua các ranh giới FFI giữa C++ và Rust:

```mermaid
sequenceDiagram
    autonumber
    actor Client
    participant HTTP as HttpHandler (C++)
    participant Core as KallistoCore (C++)
    participant FFI as FFI Bridge (C++/Rust)
    participant Rust as core_crypto (Rust)
    participant Vault as Vault Transit API

    Client->>HTTP: POST /v1/secret/data/apiKey
    HTTP->>Core: put("apiKey", secretVal)
    Note over Core: Encrypt values via BoringSSL<br/>using DEK
    Core->>FFI: Request DEK verification
    FFI->>Rust: Check KEK status
    alt KEK not unsealed
        Rust->>Vault: Decrypt/Unwrap KEK
        Vault-->>Rust: Return decrypted KEK
    end
    Rust-->>Core: Provide encrypted DEK & metadata
    Core->>Core: Commit to Sharded CuckooTable (RAM)
    Core->>Core: Push to Async Log Queue (Capacity: 262,144)
    Core-->>Client: 200 OK (Instant Response!)
    
    Note over Core: Asynchronous Disk Write-Behind
    Core->>Core: Batch flush to RocksDB (Disk)
```

### Các bước hoạt động cốt lõi
1.  **Tiếp nhận yêu cầu**: Cổng HTTP của C++ (Worker) nhận yêu cầu với tốc độ SIMD nhanh nhất.
2.  **Mã hóa phong bì**: Giá trị mật được mã hóa AES-256-GCM ở lớp C++ bằng khóa nội bộ DEK (Data Encryption Key).
3.  **Tách ranh giới bảo mật**: Khóa DEK được bao bọc bởi KEK (Key Encryption Key) được quản lý và lưu giữ an toàn bên phía Rust.
4.  **Phục hồi và ghi đĩa**: Dữ liệu ghi tức thời vào RAM Cache (CuckooTable) và đẩy vào hàng đợi ghi đĩa bất đồng bộ để lưu xuống RocksDB mà không chặn yêu cầu khách.
