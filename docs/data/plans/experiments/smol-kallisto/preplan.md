Đúng, và tôi thích ý này. Nhưng cho tôi cãi đúng **một** chỗ, vì tôi nghĩ bạn đang gọi sai tên thứ mình sắp làm.

## Đây không phải nhánh thử nghiệm. Đây là sản phẩm.

Nhớ lúc ta bàn chế độ proxy không nên ghi đĩa gì cả không? `smol-kallisto` **chính là chế độ proxy**. Bạn không đang rẽ
nhánh đi chơi — bạn đang xây đúng cái mà hai ngày qua ta kết luận là hình dạng đúng của nó.

Điều đó quan trọng ở chỗ: nếu là nhánh riêng, nó sẽ trôi xa dần rồi chết. Một người, hai codebase — kết cục đó gần như
chắc chắn. Còn nếu là **feature flag** thì `smol` là main với persistence tắt đi, cùng một cây code:

```
cargo build --no-default-features   # smol
cargo build                         # đầy đủ
```

Và đây là lý do thật sự nó quan trọng: mấy thứ ta bàn — thế hệ, eviction, chép ra trước — **là bug fix mà main cũng
cần**. Làm ở nhánh riêng thì bạn vừa sửa bug ở nhầm chỗ.

## Cái đẹp nhất của smol là những chương biến mất

Bỏ đĩa đi thì:

- Không mã hoá lúc lưu → không cần KEK → **không unseal, không Shamir, không Transit.** Cả chương đó bốc hơi. Khoá mã
  hoá trong RAM chỉ là 32 byte ngẫu nhiên sinh lúc khởi động, không bao giờ chạm đĩa. Tuổi thọ khoá = tuổi thọ tiến
  trình.
- Không write-behind → **con bug 0.92% không có chỗ để tồn tại.**
- Kill process là mọi plaintext biến mất, không để vết.

Bạn viết code ít đi mà được tính chất bảo mật mạnh hơn. Hiếm khi có deal như vậy.

## Và đây là chỗ tôi nghĩ bạn sẽ vui nhất

Trong smol, cái chỉ số kia dựng được **dễ** — vì không còn tầng persistence làm nhiễu bức tranh:

```
kallisto_plaintext_secrets_resident   142
kallisto_oldest_plaintext_age_seconds  38
```

Hai dòng đó, không sản phẩm nào tôi biết xuất ra. Và trong smol, chúng **đúng theo định nghĩa** — không có bản sao nào ở
đâu khác để mà sai.

Nếu bạn muốn một thứ để đem đi khoe cuối tuần này, tôi sẽ chọn hai dòng đó chứ không phải một con số throughput mới.

## Gợi ý phạm vi cho cuối tuần đầu

Đừng ôm cả gói. Tôi sẽ làm đúng thế này:

1. Copy engine, xoá sạch đường RocksDB và write-behind.
2. Thêm thế hệ vào ô arena.
3. Đổi đường đọc thành chép-ra-trước-diễn-giải-sau.
4. Hai chỉ số ở trên.

Hết. Mã hoá và xác thực để cuối tuần sau — chúng độc lập, và mỗi cái đủ vui riêng.

Lý do xếp thế: bước 2 và 3 là **bản nháp của bug fix cho main**. Làm ở đây dễ hơn nhiều vì không có persistence xen vào,
và khi chạy đúng thì bạn bê nguyên về. Đó mới là công dụng chính đáng của một nhánh thử nghiệm — không phải chạy trốn
main, mà là **chỗ để nghĩ cho rõ trước khi động vào code khó hơn.**