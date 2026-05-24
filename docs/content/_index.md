---
title: "Naughtian Kallisto - High Performance Hybrid Secret Engine"
layout: hextra-home
---

{{< hextra/hero-badge >}}
  <div class="hx:w-2 hx:h-2 hx:rounded-full hx:bg-primary-400"></div>
  <span>Learn more about Naughtian Kallisto's Features</span>
  {{< icon name="arrow-circle-right" attributes="height=14" >}}
{{< /hextra/hero-badge >}}

<div class="hx:mt-6 hx:mb-6">
{{< hextra/hero-headline >}}
  Naughtian Kallisto&nbsp;<br class="hx:sm:block hx:hidden" />
{{< /hextra/hero-headline >}}
</div>

<div class="hx:mb-12">
{{< hextra/hero-subtitle >}}
  Securely store and distribute operational secrets for critical secret management systems using CLI, TUI or HTTP API.
{{< /hextra/hero-subtitle >}}
</div>

<div class="hx:mb-6">
{{< hextra/hero-button text="Getting Started" link="docs" >}}
</div>

<div class="hx:mt-12"></div>

{{< hextra/feature-grid >}}
  {{< hextra/feature-card
    title="Data runtime engine by C++"
    subtitle="Lock-free, scale-per-cores, armored 2ms response latency."
    style="--card-accent-color: #2563eb;"
  >}}
  {{< hextra/feature-card
    title="Control runtime server by Rust"
    subtitle="Rust control plane server with async configuration on-the-fly."
    style="--card-accent-color: #7c3aed;"
  >}}
  {{< hextra/feature-card
    title="A Universal Data Plane for Roots of Trust"
    subtitle="Envelope Encryption KEK and upstream controlled by HashiCorp Vault/OpenBao/Infisical, ..."
    style="--card-accent-color: #059669;"
  >}}
  {{< hextra/feature-card
    title="Hot-cache High Performance"
    subtitle="Sharded Cuckoo Table lock-free data structure to enhance read/write performance up to 91,000+ RPS per core with microsecond latency."
    style="--card-accent-color: #d97706;"
  >}}
  {{< hextra/feature-card
    title="Standardized AES-256-GCM Encryption"
    subtitle="Secrets are encrypted using AES-256-GCM with hardware acceleration (AES-NI) for maximum security and performance."
    style="--card-accent-color: #dc2626;"
  >}}
  {{< hextra/feature-card
    title="Gossip & Clustering Protocol"
    subtitle="Automatic node discovery, masterless cluster using Rust `foca` library."
    style="--card-accent-color: #0891b2;"
  >}}
{{< /hextra/feature-grid >}}
