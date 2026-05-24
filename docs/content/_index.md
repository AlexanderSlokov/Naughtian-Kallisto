---
title: "Kallisto - High Performance Hybrid Secret Engine"
layout: hextra-home
---

{{< hextra/hero-badge >}}
  <div class="hx:w-2 hx:h-2 hx:rounded-full hx:bg-primary-400"></div>
  <span>OSS Project</span>
  {{< icon name="arrow-circle-right" attributes="height=14" >}}
{{< /hextra/hero-badge >}}

<div class="hx:mt-6 hx:mb-6">
{{< hextra/hero-headline >}}
  Naughtian Kallisto&nbsp;<br class="hx:sm:block hx:hidden" />
{{< /hextra/hero-headline >}}
</div>

<div class="hx:mb-12">
{{< hextra/hero-subtitle >}}
  Hệ thống quản lý thông tin mật Hybrid kết hợp hiệu năng vượt trội của C++<br class="hx:sm:block hx:hidden" />và lớp vỏ bảo mật an toàn bộ nhớ của Rust.
{{< /hextra/hero-subtitle >}}
</div>

<div class="hx:mb-6">
{{< hextra/hero-button text="Bắt Đầu" link="docs" >}}
</div>

<div class="hx:mt-12"></div>

{{< hextra/feature-grid >}}
  {{< hextra/feature-card
    title="Data runtime engine by C++"
    subtitle="Lock-free, scale-per-cores, armored 2ms response latency."
    style="background: radial-gradient(ellipse at 50% 80%,rgba(194,97,254,0.15),hsla(0,0%,100%,0));"
  >}}
  {{< hextra/feature-card
    title="Control runtime server by Rust"
    subtitle="Rust control plane server with async configuration on-the-fly."
    style="background: radial-gradient(ellipse at 50% 80%,rgba(221,210,59,0.15),hsla(0,0%,100%,0));"
  >}}
  {{< hextra/feature-card
    title="Data Plane for Roots of Trust"
    subtitle="Envelope Encryption KEK and upstream controlled by HashiCorp Vault/OpenBao/Infisical, ..."
    style="background: radial-gradient(ellipse at 50% 80%,rgba(142,53,74,0.15),hsla(0,0%,100%,0));"
  >}}
  {{< hextra/feature-card
    title="Hot-cache High Performance"
    subtitle="Sharded Cuckoo Table lock-free data structure to enhance read/write performance up to 91,000+ RPS per core with microsecond latency."
    style="background: radial-gradient(ellipse at 50% 80%,rgba(142,142,74,0.15),hsla(0,0%,100%,0));"
  >}}
  {{< hextra/feature-card
    title="Gossip & Clustering Protocol"
    subtitle="Automatic node discovery, masterless cluster using Rust `foca` library."
    style="background: radial-gradient(ellipse at 50% 80%,rgba(221,137,59,0.15),hsla(0,0%,100%,0));"
  >}}
{{< /hextra/feature-grid >}}
