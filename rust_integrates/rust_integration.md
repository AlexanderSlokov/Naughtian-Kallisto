# Kế Hoạch Tích Hợp Rust: Kiến Trúc Lai C++ & Rust (Core-Armor Pattern)

Để giữ cho kiến trúc của Naughtian-Kallisto phiên bản 2.0.0+ luôn sạch sẽ, dễ bảo trì và mở rộng, chúng ta áp dụng mô hình **Kiến trúc Lai (FFI-based Hybrid Architecture)**. Mô hình này phân tách rạch ròi hệ thống thành hai phần: **Lõi C++ (Data Plane)** gánh tải I/O siêu tốc và **Giáp Rust (Control Plane)** bảo vệ an toàn bộ nhớ và quản trị mật mã.

---

## 1. Triết Lý Thiết Kế: Data Plane vs. Control Plane

### 🔥 C++ Data Plane (Hotpath - Luồng Dữ Liệu Nóng)
Nhiệm vụ: Chịu tải hàng triệu RPS, độ trễ < 2ms, xử lý cấu trúc dữ liệu Lock-free.
- **Thành phần:** `Dispatcher`, `WorkerPool` (Epoll, SO_REUSEPORT), `HttpHandler`, `KallistoCore`, `KvEngine`, `TlsBTreeManager` (RCU), `ShardedCuckooTable`, và `RocksDBStorage`.
- **Encryption Barrier:** C++ trực tiếp gọi các thư viện tối ưu (OpenSSL/BoringSSL) để mã hóa/giải mã AES-256-GCM hàng triệu lần mỗi giây bằng Data Encryption Key (DEK) do Rust cấp.
- **Mạng lưới TLS:** TLS/mTLS termination được xử lý bằng C++ hoặc ủy thác (offload) cho Envoy Sidecar proxy.

### 🛡️ Rust Control Plane (Coldpath - Lớp Giáp An Toàn)
Nhiệm vụ: Tính toán mật mã tĩnh, quản lý bộ nhớ an toàn (anti-leak/anti-swap), giao tiếp I/O ngoại vi bất đồng bộ (Network/Disk) mà không cản trở luồng C++.
- **Thành phần:** Sinh khóa, Shamir's Secret Sharing, xoay vòng khóa Keyring, xuất Metrics, Audit Log, Gossip Protocol, quản lý ACL/RBAC, và Client TUI.

---

## 2. Các Gói (Crates) & Giải Thuật Đề Xuất

Để xây dựng các module Rust đạt chuẩn công nghiệp, chúng ta sẽ sử dụng các crate mạnh mẽ nhất từ hệ sinh thái:

### 2.1 Bảo Mật & Quản Lý Vùng Nhớ (Core Crypto)
- **Thuật toán Shamir:** `rusty_secrets` (chuẩn mực, tích hợp MAC) hoặc `vsss-rs` (có xác minh toán học Verifiable Secret Sharing).
- **Vùng Nhớ An Toàn (Secure Memory):**
  - `memsec` hoặc `secstr`: Bọc system calls như `mlock()` để cấm OS swap Master Key xuống đĩa cứng.
  - `zeroize`: Tự động ghi đè vùng RAM thành số 0 khi khóa hết hạn (chống Cold Boot Attack).
- **Ngăn Tràn Thông Tin (Anti-Leakage):** `secrecy` (Cung cấp `SecretString`, vô hiệu hóa trait `Debug` để chống in nhầm ra log `println!`).
- **Phần Cứng An Toàn (Tương lai):** `fortanix-sgx-abi` để chạy thuật toán bên trong Intel SGX Enclave.

### 2.2 Quan Sát & Nhật Ký (Telemetry & Observability)
*Chiến lược:* C++ ném dữ liệu thô vào **Lock-free Queue** cực nhanh rồi quay đi ngay. Rust dùng runtime bất đồng bộ hút dữ liệu xử lý.
- **Runtime:** `tokio` (xử lý Async I/O không chặn).
- **Metrics Exporter:** `prometheus` kết hợp với web framework siêu nhẹ `axum` hoặc `hyper` để mở port `8201` độc lập.
- **Audit Logging:** Dùng `serde_json` để parse tốc độ cao, `tracing-appender` để ghi log xuống đĩa không chặn, và `reqwest` để đẩy log lên SIEM.

### 2.3 Quản Trị Cụm & Cấu Hình (Control Plane)
- **Gossip Protocol:** `foca` (thực thi thuật toán SWIM) để tìm kiếm các node và đồng bộ cụm mạng.
- **Cấu hình:** `hcl-rs` để parse các file cấu hình `kallisto.hcl`.

### 2.4 Cầu Nối C++ & Rust (FFI Bridge)
- **Lựa chọn tối ưu:** Dùng crate **`cxx`** thay vì `cbindgen` và raw pointers truyền thống.
- **Lý do:** `cxx` tự sinh file header C++ an toàn, hỗ trợ chuyển đổi trực tiếp các kiểu dữ liệu nâng cao (`String`, `Vec`, `Result`) mà không gặp lỗi rò rỉ bộ nhớ.

### 2.5 Storage Adapter (Khả Năng Thay Thế Trong Tương Lai)
Nhờ kiến trúc Hexagonal (Storage Engine là Plug-in), nếu RocksDB gặp vấn đề, ta có thể viết Adapter gọi FFI sang hệ sinh thái Rust:
- **Các ứng viên thay thế RocksDB:** `sled` (Bw-Tree thuần Rust), `redb`, `persy`, hoặc `rust-rocksdb`.

### 2.6 TUI Client (Ứng Dụng Quản Trị)
- Giao diện Admin CLI có thể được biên dịch độc lập bằng Rust thành Single Static Binary.
- **Thư viện:** Dùng `ratatui` (vẽ giao diện terminal cực đẹp) kết hợp với `reqwest` để gọi API.

---

## 3. Tổ Chức Không Gian Làm Việc (Cargo Workspace)

Để tối ưu hóa thời gian biên dịch và chia nhỏ các ngữ cảnh Bounded Context, ta tổ chức mã nguồn dưới dạng **Monorepo**:

```text
kallisto/
├── CMakeLists.txt             <-- Sếp sòng C++ (Dùng Corrosion rs)
├── src/                       <-- C++ Core (Hotpath)
│   └── ...
└── rust_integrates/           <-- Vùng đệm an toàn của Rust (Control Plane)
    ├── Cargo.toml             <-- [workspace] root (Khai báo các members)
    │
    ├── ffi_bridge/            <-- LỚP CHỐNG THẤM (Adapter / Anti-Corruption)
    │   ├── Cargo.toml         <-- Type: staticlib (cxx-build)
    │   ├── build.rs           <-- Dùng thư viện `cxx` tự sinh header C++
    │   └── src/
    │       └── lib.rs         <-- Nơi DUY NHẤT chứa cầu nối giao tiếp C++ <-> Rust
    │
    ├── core_crypto/           <-- BẢO MẬT & VÙNG NHỚ AN TOÀN
    │   ├── Cargo.toml         
    │   └── src/
    │       ├── shamir.rs      <-- Thuật toán Unseal / Seal
    │       ├── master_key.rs  <-- Lõi quản lý vùng nhớ (mlock, no-swap, zeroize)
    │       └── rotation.rs    <-- Logic sinh DEK và xoay vòng khóa
    │
    ├── telemetry/             <-- HỆ THỐNG QUAN SÁT (Bất đồng bộ)
    │   ├── Cargo.toml         
    │   └── src/
    │       ├── metrics.rs     <-- Prometheus HTTP Server (Chạy ở thread ngầm, port 8201)
    │       └── audit_log.rs   <-- Tiêu thụ Lock-free queue từ C++ để ghi File/SIEM
    │
    ├── control_plane/         <-- QUẢN TRỊ CỤM & HỆ THỐNG
    │   ├── Cargo.toml         
    │   └── src/
    │       ├── gossip.rs      <-- Khám phá các node Kallisto khác (foca)
    │       ├── config.rs      <-- Parse kallisto.hcl
    │       └── admin_uds.rs   <-- Lắng nghe UDS nhận lệnh Admin (Mode, Flush)
    │
    ├── policy_engine/         <-- KIỂM SOÁT TRUY CẬP (ACL)
    │   ├── Cargo.toml
    │   └── src/
    │       ├── rbac.rs        <-- Parsing policy path, roles
    │       └── lease_mgr.rs   <-- Worker theo dõi và hủy các secret hết hạn
    │
    └── kallisto_tui/          <-- ỨNG DỤNG CLIENT (Tuỳ chọn đặt chung repo)
        ├── Cargo.toml         
        └── src/
            ├── main.rs        <-- Entrypoint file nhị phân độc lập
            ├── ui/            <-- Giao diện dashboard Terminal (ratatui)
            └── client.rs      <-- Call API / UDS Admin
```

---

## 4. Tích Hợp Build System & IDE

### 4.1. Sự Kết Hợp Giữa CMake và Cargo (Corrosion)
Tuyệt đối không dùng Git Submodule hay bash script tự chế. Sử dụng module `Corrosion` (Rust for CMake) trong `CMakeLists.txt` chính của Kallisto:

```cmake
# Tải Corrosion
include(FetchContent)
FetchContent_Declare(
    Corrosion
    GIT_REPOSITORY https://github.com/corrosion-rs/corrosion.git
    GIT_TAG v0.4.2
)
FetchContent_MakeAvailable(Corrosion)

# Yêu cầu CMake biên dịch toàn bộ Rust Workspace
corrosion_import_crate(MANIFEST_PATH rust_integrates/ffi_bridge/Cargo.toml)

# Link thư viện tĩnh Rust vào Kallisto C++ Core
target_link_libraries(kallisto_core PUBLIC ffi_bridge)
```

Khi chạy `make build-server`, CMake sẽ tự động đánh thức Cargo biên dịch Rust workspace thành file `.a`, sau đó link chung với C++ object files thành một file nhị phân duy nhất.

### 4.2. Cấu Hình IDE (CLion)
Không cần tải thêm IDE chuyên biệt cho Rust (như RustRover) gây nặng máy.
- Mở thư mục gốc `kallisto/` bằng **CLion**.
- Cài plugin **Rust** chính thức của JetBrains.
- CLion sẽ tự động nhận diện cả `CMakeLists.txt` (C++) và `Cargo.toml` (Rust). Bạn có thể dễ dàng dùng tính năng Go-To-Definition (`Ctrl+Click`) để nhảy xuyên thủng ranh giới FFI giữa hai ngôn ngữ một cách mượt mà.
