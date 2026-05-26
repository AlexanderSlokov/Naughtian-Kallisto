---
title: "Đánh giá Unsafe & Thiết kế An toàn"
weight: 20
---

# Đánh giá chuyên sâu: Phân tích Unsafe & Thiết kế An toàn cho `admin_http.rs`

Tài liệu này cung cấp một bài đánh giá chuyên sâu, mang tính sống còn đối với môi trường sản xuất (production-critical) về các mẫu mã nguồn không an toàn (unsafe code patterns) bên trong cầu nối FFI cũ tại `components/kallisto_cluster/src/admin_http.rs`. Báo cáo sẽ đánh giá các rủi ro hiện tại và phác thảo một thiết kế tái cấu trúc thuần Rust an toàn 100%, được hiện thực hóa nhờ vào quá trình rewrite thuần Rust của tầng data plane.

---

### 1. Tóm tắt vấn đề
Trong lịch sử, Kallisto đã sử dụng kiến trúc lai (hybrid) C++/Rust, trong đó Tầng Dữ liệu (Data Plane) hiệu năng cao được viết bằng C++20, còn Tầng Điều khiển (Control Plane / Gossip) được viết bằng Rust.

File `admin_http.rs` được thiết kế như một **cầu nối FFI (FFI adapter bridge)** cho phép máy chủ HTTP quản trị viết bằng Axum (phía Rust) kích hoạt các hành động quản trị (như ghi dữ liệu xuống đĩa hoặc thay đổi chế độ đồng bộ) bên trong runtime C++.

Để đạt được luồng điều khiển xuyên ngôn ngữ này:
- C++ chuyển con trỏ lõi thô (`*mut c_void`) của nó sang Rust.
- C++ chuyển các con trỏ hàm không an toàn kiểu C (`AdminCallbacks`) sang Rust.
- Rust phải bỏ qua các cơ chế đảm bảo an toàn kiểu dữ liệu (type safety), an toàn đa luồng (thread safety) và quản lý bộ nhớ tiêu chuẩn bằng cách sử dụng các con trỏ thô và ghi đè bằng từ khóa `unsafe`.

Với **kế hoạch Rewrite thuần Rust** (đã hoàn thành Phase 1 & 2), tầng dữ liệu C++ cũ đã hoàn toàn được thay thế bằng một công cụ `KvEngine` nguyên bản, an toàn và hỗ trợ đa luồng cực kỳ mạnh mẽ của Rust. Nhu cầu về các cầu nối FFI, C callbacks và con trỏ thô hiện nay đã **hoàn toàn lỗi thời**.

---

### 2. Phân tích chi tiết mã nguồn Unsafe & Các rủi ro hệ thống

Mỗi khối `unsafe` trong `admin_http.rs` đều ẩn chứa những rủi ro nghiêm trọng về độ tin cậy, bảo mật và khả năng bảo trì:

#### A. Ghi đè An toàn Đa luồng Thủ công (Dòng 11-15)
```rust
#[derive(Clone)]
pub struct SafeCorePointer(pub *mut std::ffi::c_void);

unsafe impl Send for SafeCorePointer {}
unsafe impl Sync for SafeCorePointer {}
```
- **Tại sao nó tồn tại**: Các con trỏ thô (raw pointers) mặc định không được gắn trait `Send` hoặc `Sync` vì trình biên dịch không thể xác minh xem liệu nhiều luồng có thể đọc/ghi an toàn vào vùng nhớ được trỏ tới hay không.
- **Rủi ro chí mạng**: Đây là một lời hứa trực tiếp với trình biên dịch ("Hãy tin tôi, con trỏ này an toàn khi chia sẻ giữa các luồng"). Nếu đối tượng C++ bên dưới không an toàn đa luồng (ví dụ: thực hiện thay đổi trạng thái non-atomic mà không có cơ chế đồng bộ hóa), điều này sẽ kích hoạt **hành vi không xác định (Undefined Behavior - UB)**, tranh chấp dữ liệu (data race) và làm hỏng bộ nhớ (memory corruption) tại thời điểm chạy mà trình biên dịch Rust hoàn toàn bị "bịt mắt".

#### B. Con trỏ hàm Unsafe kiểu C (Dòng 17-20)
```rust
pub struct AdminCallbacks {
    pub force_flush: unsafe fn(*mut std::ffi::c_void) -> bool,
    pub change_sync_mode: unsafe fn(*mut std::ffi::c_void, i32) -> bool,
}
```
- **Tại sao nó tồn tại**: Cho phép gọi ngược (call back) vào mã nguồn đã được biên dịch của C++ thông qua các địa chỉ hàm thô.
- **Rủi ro chí mạng**: Việc giải tham chiếu các con trỏ hàm tùy ý bỏ qua hoàn toàn bộ kiểm tra mượn (borrow checker) và chữ ký hàm của Rust. Nếu C++ engine đã bị hủy bỏ (lỗi use-after-free) hoặc con trỏ bị lỗi/corrupted, việc gọi hàm này sẽ lập tức kích hoạt lỗi **Segmentation Fault** hoặc nhảy luồng thực thi đến một địa chỉ bộ nhớ ngẫu nhiên (đây là vector tấn công chính cho các khai thác chiếm quyền điều khiển luồng thực thi - control-flow hijack).

#### C. Quản lý Bộ nhớ Heap Thủ công qua Con trỏ (Dòng 81-102)
```rust
// Trong start_admin_server:
Box::into_raw(server)

// Trong stop_admin_server:
unsafe {
    let server = Box::from_raw(server_ptr);
    ...
}
```
- **Tại sao nó tồn tại**: Bỏ qua trình theo dõi vòng đời tự động của Rust (RAII) để chuyển quyền sở hữu của một cấu trúc Rust (`AdminServer`) sang C++ dưới dạng một con trỏ thô.
- **Rủi ro chí mạng**: Nhiệm vụ quản lý bộ nhớ bị đẩy sang cho C++. Nếu C++ quên không gọi `stop_admin_server`, bộ nhớ sẽ bị rò rỉ (memory leak) vĩnh viễn. Ngược lại, nếu C++ gọi hàm này hai lần, nó sẽ kích hoạt lỗi **giải phóng bộ nhớ hai lần (double-free)**, làm hỏng cấu trúc heap và khiến toàn bộ máy chủ bị sập ngay lập tức.

#### D. Các Endpoint giải tham chiếu Con trỏ Lõi (Dòng 104-129)
```rust
let success = unsafe { (state.callbacks.force_flush)(state.core_ptr.0) };
```
- **Tại sao nó tồn tại**: Kích hoạt hàm callback mỗi khi có một request HTTP gửi tới `/admin/save` hoặc `/admin/mode/*`.
- **Rủi ro chí mạng**: Cực kỳ nhạy cảm với các lỗi bất đồng bộ. Nếu endpoint `/admin/save` bị bắn phá bởi hàng loạt request đồng thời, tất cả chúng sẽ cố gắng giải tham chiếu cùng một con trỏ lõi C++ thô tại cùng một thời điểm, gây ra lỗi tranh chấp nghiêm trọng nếu phía C++ thiếu các cơ chế khóa mutex nội bộ chặt chẽ.

---

### 3. Thiết kế lại hướng tới An toàn 100% (Không còn Unsafe)

Vì toàn bộ data plane hiện tại đã là Rust thuần túy, chúng ta có thể loại bỏ hoàn toàn mọi dấu vết của `unsafe` bằng cách sử dụng các trait tiêu chuẩn của Rust, các con trỏ thông minh (smart pointers) và các mô hình async bất đồng bộ.

#### Bước 1: Định nghĩa một Safe Control Trait
Thay vì sử dụng các con trỏ thô và callbacks kiểu cũ, chúng ta định nghĩa một trait Rust tiêu chuẩn và an toàn để biểu diễn các hành động quản trị engine:

```rust
use crate::engine::error::EngineError;
use crate::engine::kv_engine::SyncMode;

pub trait EngineController: Send + Sync {
    fn force_flush(&self) -> Result<(), EngineError>;
    fn change_sync_mode(&self, mode: SyncMode);
}
```

Chúng ta có thể dễ dàng triển khai trait này cho `KvEngine` (hoặc `EngineRegistry`):

```rust
impl EngineController for KvEngine {
    fn force_flush(&self) -> Result<(), EngineError> {
        // Gọi lệnh flush an toàn của RocksDB
        self.rocksdb.flush().map_err(|e| EngineError::StorageError(e.to_string()))
    }

    fn change_sync_mode(&self, mode: SyncMode) {
        self.change_sync_mode(mode);
    }
}
```

#### Bước 2: Viết lại `AdminServer` một cách an toàn
Sử dụng Axum kết hợp với `Arc<dyn EngineController>` tiêu chuẩn và điều phối tác vụ async:

```rust
use axum::{
    extract::State,
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::sync::oneshot;

#[derive(Clone)]
struct AppState {
    engine: Arc<dyn EngineController>,
}

pub struct AdminServer {
    shutdown_tx: oneshot::Sender<()>,
}

impl AdminServer {
    pub async fn start(
        engine: Arc<dyn EngineController>,
        port: u16,
    ) -> Result<Self, std::io::Error> {
        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
        let state = AppState { engine };

        let app = Router::new()
            .route("/admin/save", post(handle_save))
            .route("/admin/mode/batch", post(handle_mode_batch))
            .route("/admin/mode/immediate", post(handle_mode_immediate))
            .route("/admin/status", get(handle_status))
            .with_state(state);

        let addr = format!("127.0.0.1:{}", port);
        let listener = tokio::net::TcpListener::bind(&addr).await?;
        println!("[Rust Admin Server] Listening on http://{}", addr);

        // Spawn trực tiếp Axum server như một task lên trên Tokio runtime hiện tại
        tokio::spawn(async move {
            let server = axum::serve(listener, app)
                .with_graceful_shutdown(async move {
                    let _ = shutdown_rx.await;
                    println!("[Rust Admin Server] Shutdown signal received.");
                });
            if let Err(e) = server.await {
                eprintln!("[Rust Admin Server] Server error: {}", e);
            }
        });

        Ok(Self { shutdown_tx })
    }

    pub fn stop(self) {
        let _ = self.shutdown_tx.send(());
    }
}
```

#### Bước 3: Triển khai các Endpoint Handler an toàn
Không còn bất kỳ khối `unsafe` nào, lỗi được định nghĩa rõ ràng có cấu trúc và được xác thực hoàn toàn ngay tại thời điểm biên dịch!

```rust
async fn handle_save(State(state): State<AppState>) -> (StatusCode, Json<Value>) {
    match state.engine.force_flush() {
        Ok(_) => (
            StatusCode::OK,
            Json(json!({"status": "OK", "message": "Database flushed to disk."})),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"errors": [format!("Failed to flush database: {}", e)]})),
        ),
    }
}

async fn handle_mode_batch(State(state): State<AppState>) -> (StatusCode, Json<Value>) {
    state.engine.change_sync_mode(SyncMode::Batch);
    (
        StatusCode::OK,
        Json(json!({"status": "OK", "message": "Mode changed to BATCH."})),
    )
}

async fn handle_mode_immediate(State(state): State<AppState>) -> (StatusCode, Json<Value>) {
    state.engine.change_sync_mode(SyncMode::Immediate);
    (
        StatusCode::OK,
        Json(json!({"status": "OK", "message": "Mode changed to IMMEDIATE."})),
    )
}

async fn handle_status() -> (StatusCode, Json<Value>) {
    (
        StatusCode::OK,
        Json(json!({
            "status": "OK",
            "version": env!("CARGO_PKG_VERSION"),
            "features": {
                "control_plane_port": 8202,
                "data_plane_port": 8200
            }
        })),
    )
}
```

---

### 4. Đánh giá Chi phí và Rủi ro

| Khía cạnh | Kiến trúc lai cũ (Unsafe) | Thiết kế mới đề xuất (An toàn) | Mức độ cải thiện |
|---|---|---|---|
| **Độ chính xác** | Nguy cơ lỗi UB rất cao do lập trình viên tự bảo đảm an toàn đa luồng. | Được xác thực 100% tại thời điểm biên dịch bởi bộ kiểm tra mượn (borrow checker) của Rust. | **An toàn tuyệt đối** |
| **Bảo mật** | Dễ bị tấn công tràn bộ đệm, lỗi nhảy luồng và các khai thác bộ nhớ FFI. | Miễn dịch hoàn toàn với các lỗi khai thác bộ nhớ nhờ cơ chế bảo vệ an toàn của Rust. | **Khắc phục cực kỳ quan trọng** |
| **Độ mạnh mẽ** | Nguy cơ rò rỉ bộ nhớ hoặc double-free nếu các luồng gọi FFI bị mất đồng bộ. | Đảm bảo dọn dẹp tài nguyên tự động thông qua cơ chế RAII tiêu chuẩn khi đối tượng bị drop. | **Không rò rỉ bộ nhớ** |
| **Gánh nặng tư duy** | Rất cao. Lập trình viên phải tự quản lý vòng đời và tính toán con trỏ thô. | Thấp. Code Rust bất đồng bộ tiêu chuẩn, dễ đọc, dễ hiểu. | **Giảm tải cực lớn** |
| **Hiệu năng / CPU** | Gần như tương đương. Việc gọi qua Rust trait object có chi phí cực kỳ nhỏ (~2ns cho virtual call so với gọi qua con trỏ thô). | Gần như tương đương. Máy chủ Axum chạy trực tiếp trên runtime chính mà không cần tạo thêm luồng runtime phụ. | **Bằng nhau** |

---

### 5. Khuyến nghị
**Có, chúng ta hoàn toàn có thể và thực sự nên loại bỏ hoàn toàn các khối `unsafe`.**

Bằng cách đóng gói các lệnh điều khiển công cụ quản trị vào trong một trait `EngineController` an toàn và truyền nó qua con trỏ thông minh `Arc<dyn EngineController>` trực tiếp vào Axum:
1. Chúng ta đạt được **0 dòng code unsafe** trong toàn bộ component quản trị cụm.
2. Chúng ta loại bỏ được luồng chạy ngầm dư thừa đang tự spawn một Tokio runtime trùng lặp (chúng ta chỉ việc chạy trực tiếp Axum task trên Tokio runtime sẵn có của tiến trình chính).
3. Chúng ta bảo vệ các API quản trị của Kallisto khỏi các lỗi nghiêm trọng như double-free, memory leak và sập tiến trình do lỗi FFI.

Quá trình refactor này có thể được thực hiện cực kỳ mượt mà trong **Phase 4 (Control Plane & Telemetry Migration)** thuộc kế hoạch tổng thể của chúng ta.
