# syntax=docker/dockerfile:1
#
# ADR-0015 D12: a static musl binary on a distroless base. The old image was an
# Ubuntu with a dynamically linked server on it, because RocksDB made anything
# smaller impractical. RocksDB is gone; what remains that needs a C toolchain is
# aws-lc-rs, which cross-compiles to musl cleanly.

FROM rust:slim AS builder
WORKDIR /app

# cmake and clang are for aws-lc-rs, which compiles C and assembly. They are the
# only native build dependency left — libssl-dev is gone with the engine, since
# every TLS connection here goes through rustls on aws-lc-rs rather than OpenSSL
# (see the comment in deny.toml).
RUN apt-get update && \
    apt-get install -y --no-install-recommends \
    cmake \
    clang \
    make \
    musl-tools \
    && rm -rf /var/lib/apt/lists/*

# The toolchain file comes first, and the musl target is added *after* it.
# Order matters and the wrong one fails late: `rustup target add` applies to the
# toolchain that is active when it runs, so adding the target before copying
# `rust-toolchain.toml` installs it against stable — then cargo reads the file,
# switches to the pinned nightly, and the build dies with
# "can't find crate for `core`" some minutes later.
COPY rust-toolchain.toml ./
RUN rustup target add x86_64-unknown-linux-musl

COPY . .

ENV CC_x86_64_unknown_linux_musl=clang \
    AR_x86_64_unknown_linux_musl=llvm-ar \
    CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_RUSTFLAGS="-Ctarget-feature=+crt-static"

RUN cargo build --release --target x86_64-unknown-linux-musl \
        -p kallisto-server -p kallisto-ctl

FROM debian:bookworm-slim AS tester
WORKDIR /app

COPY --from=builder /usr/local/cargo /usr/local/cargo
COPY --from=builder /usr/local/rustup /usr/local/rustup

ENV RUSTUP_HOME=/usr/local/rustup \
    CARGO_HOME=/usr/local/cargo \
    PATH=/usr/local/cargo/bin:$PATH

COPY --from=builder /app /app

RUN apt-get update && \
    apt-get install -y --no-install-recommends \
    ca-certificates \
    make \
    curl \
    cmake \
    clang \
    && rm -rf /var/lib/apt/lists/*

CMD ["cargo", "test", "--workspace"]

# Distroless: no shell, no package manager, nothing to pivot to. The binary is
# static, so `static-debian12` rather than `cc-debian12`.
FROM gcr.io/distroless/static-debian12:nonroot AS production

COPY --from=builder \
     /app/target/x86_64-unknown-linux-musl/release/kallisto-server \
     /usr/local/bin/kallisto-server
COPY --from=builder \
     /app/target/x86_64-unknown-linux-musl/release/kallisto-ctl \
     /usr/local/bin/kallisto-ctl

# Only 8200, and only ever reachable from inside the pod. ADR-0015's first
# operational red line is that this port is never exposed beyond localhost;
# `Config::resolve` refuses a non-loopback bind without an explicit risk flag,
# so this EXPOSE is documentation of the container's own interface, not an
# invitation to publish it. Port 8202 is gone with the admin server.
EXPOSE 8200

USER nonroot
ENTRYPOINT ["/usr/local/bin/kallisto-server"]
