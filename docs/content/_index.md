---
title: "Kallisto - High Performance Hybrid Secret Engine"
layout: hextra-home
---

{{< hextra/hero-badge >}}
  <div class="hx:w-2 hx:h-2 hx:rounded-full hx:bg-primary-400"></div>
  <span>Mã nguồn mở, hiệu năng cực đại</span>
  {{< icon name="arrow-circle-right" attributes="height=14" >}}
{{< /hextra/hero-badge >}}

<div class="hx:mt-6 hx:mb-6">
{{< hextra/hero-headline >}}
  Kallisto Secret Engine&nbsp;<br class="hx:sm:block hx:hidden" />Bảo mật tuyệt đối, Tốc độ tối đa
{{< /hextra/hero-headline >}}
</div>

<div class="hx:mb-12">
{{< hextra/hero-subtitle >}}
  Hệ thống quản lý thông tin mật Hybrid kết hợp hiệu năng vượt trội của C++<br class="hx:sm:block hx:hidden" />và lớp vỏ bảo mật an toàn bộ nhớ của Rust.
{{< /hextra/hero-subtitle >}}
</div>

<div class="hx:mb-6">
{{< hextra/hero-button text="Bắt Đầu Xem Tài Liệu" link="docs" >}}
</div>

<div class="hx:mt-12"></div>

{{< hextra/feature-grid >}}
  {{< hextra/feature-card
    title="Kiến trúc Hybrid C++/Rust"
    subtitle="Data Plane viết bằng C++ xử lý I/O lock-free siêu tốc, kết hợp Control Plane viết bằng Rust quản lý KEK an toàn bộ nhớ."
    style="background: radial-gradient(ellipse at 50% 80%,rgba(194,97,254,0.15),hsla(0,0%,100%,0));"
  >}}
  {{< hextra/feature-card
    title="Vault Transit làm Root of Trust"
    subtitle="Mã hóa phong bì (Envelope Encryption) chuẩn doanh nghiệp. Master Key nằm an toàn tại Vault, KEK tự động hủy khỏi RAM (zeroize) ngay khi tắt."
    style="background: radial-gradient(ellipse at 50% 80%,rgba(142,53,74,0.15),hsla(0,0%,100%,0));"
  >}}
  {{< hextra/feature-card
    title="Tìm Kiếm Toàn Văn Bản Siêu Tốc"
    subtitle="Tính năng tìm kiếm FlexSearch offline tích hợp sẵn giúp nhà phát triển tìm kiếm tài liệu tức thời mà không cần thiết lập phức tạp."
    style="background: radial-gradient(ellipse at 50% 80%,rgba(221,210,59,0.15),hsla(0,0%,100%,0));"
  >}}
  {{< hextra/feature-card
    title="Hiệu năng Hot-cache cực đỉnh"
    subtitle="Sử dụng cấu trúc dữ liệu Sharded Cuckoo Table lock-free nâng hiệu suất đọc ghi lên tới hơn 91,000+ RPS với độ trễ microsecond."
  >}}
  {{< hextra/feature-card
    title="Giao thức Gossip & Clustering"
    subtitle="Khả năng tự động khám phá nút mạng trong cụm bằng thuật toán SWIM/foca viết bằng Rust giúp đồng bộ trạng thái cực nhanh."
  >}}
  {{< hextra/feature-card
    title="Giao diện Dark Mode Zen-Dark"
    subtitle="Mang lại trải nghiệm thị giác cao cấp, dễ chịu nhất cho các kỹ sư lập trình giống như trang tài liệu của HashiCorp."
  >}}
{{< /hextra/feature-grid >}}
