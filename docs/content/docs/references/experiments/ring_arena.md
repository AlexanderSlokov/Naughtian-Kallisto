Sau này nếu có chuyển thành arena vòng. Lúc đó "đầy" không còn là một trạng thái tồn tại.
Con trỏ chạy hết vòng thì đè lên cái cũ nhất, thế là xong: FIFO miễn phí, và ô cũ bị ghi đè vật lý nên tự lau plaintext.

Chi tiết kỹ thuật bạn sẽ cần cho bước đó, nói luôn kẻo lúc code mới phát hiện: khi con trỏ vòng quay tới ô N để dùng lại,
bạn phải xoá được entry tương ứng trong cuckoo table, nếu không bảng đầy dần bằng các con trỏ trỏ vào ô đã bị người khác chiếm.
Nên mỗi ô arena cần một cái đầu nhỏ chứa băm của key: đọc băm → xoá khỏi bảng → tăng thế hệ → ghi đè.
Bitcask dùng đúng thủ thuật này. Tốn 8 byte một ô, và bạn có eviction O(1) không cần quét gì.

Ô arena lúc đó trông như: [thế hệ: u32][băm key: u64][độ dài: u32][dữ liệu...].
Một cấu trúc đó bịt cùng lúc: ABA, eviction, và zeroize.