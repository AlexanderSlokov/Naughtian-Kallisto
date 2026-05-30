# Đặc tả Cơ chế Master Key & Shamir Secret Sharing của Vault

Tài liệu này mô tả chi tiết quá trình Kallisto khởi tạo Master Key (Root Key) và chia nhỏ nó theo thuật toán Shamir's Secret Sharing. 

---

## 1. Khởi tạo Master Key (Root Key)

- **Thuật toán sinh khóa:** Rút trích entropy ngẫu nhiên từ hệ điều hành bằng `crypto/rand` (tương đương với `/dev/urandom` trên Linux).
- **Kích thước khóa:** Master Key là một chuỗi ngẫu nhiên dài 256-bit (32 bytes).
- **Vai trò:** Khóa này không trực tiếp mã hóa dữ liệu người dùng, mà dùng để mã hóa "vòng chìa khóa" (Keyring) của hệ thống Security Barrier. Keyring mới chứa các khóa mã hóa thật sự (Encryption Keys) dùng cho AES-GCM.

---

## 2. Giải thuật Shamir's Secret Sharing (Cắt Khóa)

### Nguyên lý Toán học
Thuật toán hoạt động dựa trên toán học về đa thức (Polynomials) trên một trường hữu hạn Galois Field - cụ thể là **GF(2^8)**. Việc dùng trường GF(2^8) thay vì số thực giúp đảm bảo không có sai số làm tròn, và mọi phép toán (cộng, nhân, chia) trên 1 byte (0-255) luôn trả về 1 byte.

### Thuật toán Chia Khóa (Split)
Đầu vào gồm:
- Mật khẩu gốc `secret` (32 bytes Master Key).
- Tổng số phần `parts` (ví dụ: `n = 5`).
- Số phần tối thiểu để ghép `threshold` (ví dụ: `k = 3`).

Các bước thực thi:
1. **Xác định tọa độ X:** Khởi tạo ngẫu nhiên một mảng hoán vị gồm các số từ 1 đến 255 làm phân phối tọa độ X. Mỗi Share (phần cắt) sẽ được gán một giá trị X cố định.
2. **Khởi tạo mảng đầu ra:** Hàm chuẩn bị `parts` mảng byte. Lưu ý: Mảng byte của Share luôn dài hơn Master Key **1 byte**. Byte cuối cùng này được dùng làm "Tag" chứa giá trị tọa độ X của chính phần chia đó (offset).
3. **Cắt từng Byte của Master Key:** Vì không gian GF(2^8) chỉ biểu diễn được giá trị từ 0-255, mật khẩu gốc 32 byte không thể dùng chung 1 đa thức. Dev phải duyệt qua từng byte của Khóa.
   - Với **mỗi byte thứ `i`** trong Master Key, đóng vai trò là "intercept" (hệ số tự do ở gốc tọa độ, tức f(0) = giá trị byte đó).
   - Dev sinh ra một đa thức ngẫu nhiên bậc `threshold - 1`. (Tại ví dụ k=3 thì là đa thức bậc 2: f(x) = ax^2 + bx + c với `c` là byte của Master Key). Các hệ số `a, b` được random dùng `crypto/rand`.
   - Với mỗi Share từ 1 đến `parts`: tính giá trị `y = f(x)` bằng phương pháp chia Horner thông qua phép cộng và nhân trong không gian ma trận GF(2^8). Gán `y` vào vị trí byte `i` của Share đó.
1. **Kết quả:** Trả về `parts` mảng mã rời rạc. Bản thân thuật toán Shamir chứng minh rằng: Biết dưới `threshold` điểm y không mang lại một chút thông tin nào về f(0) (Perfect Secrecy).
---
## 3. Mã hóa bổ sung bằng PGP (Tùy chọn)

Nếu lúc khởi tạo (Init) người dùng cung cấp danh sách PGP Public Keys, Kallisto sẽ đi qua một bước nữa:
1. Encode từng Share thô bằng `Hex Encoding` thành chuỗi ký tự.
2. Dùng thuật toán PGP mã hóa từng Share Hex theo từng Public Key tương ứng. Đảm bảo nhân sự chỉ có thể giải mã ra Share thật bằng PGP Private Key của chính họ.

---
## 4. Quá trình Lắp Khóa (Unseal/Combine)

Khi người dùng cung cấp các Unseal Key để mở khóa Vault:

1. **Chuẩn bị đầu vào:** Thu thập tối thiểu `threshold` phần Share. Kiểm tra độ dài tất cả các Share phải bằng nhau (33 bytes) và không được trùng lặp tọa độ X.
2. **Trích xuất tọa độ X:** Byte cuối cùng của mỗi Share chính là tọa độ điểm X. Dev lấy ra mảng `X_samples`.
3. **Nội suy Lagrange (Lagrange Interpolation):**
   - Với từng vị trí byte (từ 0 đến 31), thu thập tập hợp giá trị Y tương ứng của các Share thành mảng `Y_samples`.
   - Chạy công thức nội suy Lagrange trong trường hữu hạn **GF(2^8)**. Bản chất lúc này ta đi tìm giá trị tự do (f(0)). Mặc dù có công thức khôi phục toàn bộ đa thức, ta chỉ quan tâm `x = 0`.
   - Thuật toán Lagrange sẽ tính toán và tra cứu nghịch đảo bằng cơ chế `ConstantTimeSelect` để tránh rò rỉ bảo mật timing-attacks.
   - Kết quả thu được của phương trình f(0) tại vị trí đó chính là 1 byte của Master Key gốc.

Sau khi ghép đủ 32 byte, Master Key gốc được khôi phục. Sau đó, hệ thống dùng Master Key này để giải mã file `core/keyring` (bằng AES-GCM) để lấy vòng chìa khóa và bắt đầu cho phép đọc/ghi dữ liệu.

---

## Gợi ý Dev
- Lực lượng nòng cốt cho hệ thống này là triển khai đúng các hàm toán học cộng, nhân, chia, nghịch đảo trong trường hữu hạn **GF(2^8)**. Phép cộng (và trừ) thực chất chỉ là bitwise `XOR` (`a ^ b`).
- Các phép tính trên mảng bộ nhớ chứa mã khóa tuyệt đối phải sử dụng `subtle.ConstantTimeEq/Select` hoặc viết sao cho thời gian chạy độc lập với giá trị đầu vào để triệt tiêu The Timing Attacks.
- Luôn `memzero` dọn dẹp biến tạm chứa Master key trên RAM sau khi đã sử dụng (Vault dùng hàm riêng `defer memzero(buf)` sau khi load dữ liệu nhạy cảm xong).
