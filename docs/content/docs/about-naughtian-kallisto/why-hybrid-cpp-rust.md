---
title: "Tại sao lại là Hybrid C++/Rust?"
weight: 4
---

Một câu hỏi thường gặp khi mọi người xem thiết kế kiến trúc của Naughtian Kallisto là: *"Tại sao không viết Kallisto lại toàn bộ bằng Rust để đảm bảo an toàn bộ nhớ 100%?"*. 

Đối với tôi nói riêng và với dự án Kallisto nói chung, quyết định duy trì một **Data Plane bằng C++** và một **Control Plane bằng Rust** là một quyết định kiến trúc mang tính đánh đổi chiến lược (Strategic Trade-off) với các lý do rất đặc thù như sau:

## 1. Giới hạn của Rust trong lập trình hệ thống cực hạn (Extreme Systems Programming)

Dù Rust cung cấp cơ chế an toàn bộ nhớ tuyệt vời thông qua `Ownership` và `Borrow Checker`, nó lại tạo ra rào cản lớn khi hệ thống cần thực hiện những kỹ thuật thao tác bộ nhớ phức tạp để tối ưu hiệu năng, thường được gọi là "ma thuật" (magic tricks) ở tầng thấp: RCU (Read-Copy-Update) và Pointer Swapping. 

Lõi webserver và các cấu trúc dữ liệu in-memory của `Naughtian Kallisto` (Btree, CuckooHash,..) phụ thuộc rất nhiều vào các kỹ thuật hoán đổi con trỏ không khóa (lock-free pointer swapping) và mô hình **Many-Read-Many-Write** của các `worker` threads. Trong C++, ta có thể sử dụng `std::atomic` kết hợp với các chỉ thị thứ tự bộ nhớ (`std::memory_order_relaxed`, `acquire/release`) một cách hoàn toàn tự do để ép phần cứng (CPU) và Kernel làm việc với hiệu năng tối đa. Để tái hiện lại các cấu trúc dữ liệu không khóa tương tự trong Rust, ta bắt buộc phải vô hiệu hóa `Borrow Checker` bằng cách đặt toàn bộ logic lõi vào trong các khối `unsafe {}`. Khi lớp giao tiếp trực tiếp với Kernel và FFI (Foreign Function Interface) đầy rẫy `unsafe`, Rust mất đi lợi thế an toàn vốn có, nhưng lại không mang lại sự linh hoạt như C++.

Tức là: bạn vẫn không đạt được mục tiêu là "tiếp xúc với phần cứng bằng Rust một cách an toàn như Rust hứa", trong khi vẫn phải đối mặt với rủi ro về an toàn bộ nhớ. Có thể xem cách mà các kỹ sư C++ làm việc với các cấu trúc lock-free và thao tác bộ nhớ, ta có nguyên mẫu, ta có một `pattern` đã được chứng minh là hoạt động hiệu quả. Rust thì... hên xui, cách mà mấy ông kỹ sư CloudFlare handle unsafe rất khác so với cách mấy ông contributor của Linux tích hợp Rust vào lõi kernel.  

Do đó, Rust rất hay và rất tuyệt vời cho các ứng dụng vốn cần an toàn bộ nhớ lúc chạy, nhưng bản thân tụi nó không phụ thuộc vào các cơ thế tối ưu thủ công đến mức cực đoan như cách mà Kallisto yêu cầu.  

## 2. Tính hỗn mang của hệ sinh thái Cargo (Cargo Registry Chaos)

Một vấn đề lớn khác đến từ chuỗi cung ứng phần mềm (Software Supply Chain) của Rust: Hệ sinh thái Cargo (chợ package của Rust) hiện đang phát triển quá nhanh, dẫn đến tình trạng bát nháo và thiếu tiêu chuẩn hóa ở một số thư viện lõi. Việc kéo hàng chục, hàng trăm dependency con (transitive dependencies) không rõ nguồn gốc vào một phần mềm lõi về **Lưu trữ bí mật vận hành (Operational secret management)** như Kallisto là một rủi ro tấn công chuỗi cung ứng không thể chấp nhận được.

Ngược lại, C++ (thông qua vcpkg/CMake) cho phép kiểm soát ranh giới thư viện bên ngoài chặt chẽ hơn, ưu tiên sử dụng các thư viện đã được thử lửa ở cấp công nghiệp (như BoringSSL hay RocksDB) thay vì các thùng (crates) mới nổi chưa được kiểm toán bảo mật (Security Audit).

---

## 3. Nguồn dữ liệu code C++ và kiến thức của AI Agent

## 4. Lấy cái gì để biết mình code Rust đang chạy đúng? Không thể biết được, trừ phi có C++ làm tiêu chí đánh giá (benchmarks)

Nếu ta thao tác bộ nhớ thủ công với C++, ta sẽ có được đặc quyền là "làm mọi thứ tùy ý để đạt hiệu năng cực hạn, bất chấp an toàn bộ nhớ khi vận hành". Việc lấy "C++ thuần túy, không abstraction" làm cột mốc 0% thiệt hại (The Absolute Zero) sẽ giúp bạn nhìn thấu được mọi "thuế hiệu năng" (Performance Tax) mà các ngôn ngữ khác bắt bạn phải trả: 

- Nếu Rust chỉ ăn mất 2% - 5% hiệu năng: Duyệt! Đó là mức phí bảo hiểm quá rẻ để đổi lấy một hệ thống không bao giờ biết đến Segfault, giúp bạn ngủ ngon mỗi đêm.

- Nếu Rust ăn mất 15% - 20% hiệu năng hoặc dải `P99.9 Latency` tăng đột biến: Ta biết ngay code Rust ở Data Plane là vấn đề chứ không phải là Rust chỉ đạt được như vậy, và ta bị rơi vào ảo tưởng rằng "trade-off mà chúng ta đang phải chịu là chấp nhận được".