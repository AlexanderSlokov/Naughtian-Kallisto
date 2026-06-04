# syntax=docker/dockerfile:1
FROM rust:slim AS builder
WORKDIR /app
# Install necessary build dependencies
RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config libssl-dev cmake clang make \
    && rm -rf /var/lib/apt/lists/*
COPY . .
RUN cargo build --release --all

FROM ubuntu:24.04 AS tester
WORKDIR /app
COPY --from=builder /usr/local/cargo /usr/local/cargo
COPY --from=builder /usr/local/rustup /usr/local/rustup
ENV RUSTUP_HOME=/usr/local/rustup \
    CARGO_HOME=/usr/local/cargo \
    PATH=/usr/local/cargo/bin:$PATH
COPY --from=builder /app /app
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates make curl pkg-config libssl-dev cmake clang \
    && rm -rf /var/lib/apt/lists/*
CMD ["cargo", "test", "--all"]

FROM ubuntu:24.04 AS production
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*
RUN groupadd kallisto && useradd -r -g kallisto -s /bin/bash kallisto
RUN mkdir -p /kallisto/logs /kallisto/config /kallisto/data /var/run/kallisto \
    && chown -R kallisto:kallisto /kallisto /var/run/kallisto
VOLUME ["/kallisto/logs", "/kallisto/data"]
WORKDIR /app
COPY --from=builder /app/target/release/kallisto-server /app/kallisto_server
RUN chown kallisto:kallisto /app/kallisto_server
USER kallisto
EXPOSE 8200 8202
ENTRYPOINT ["/app/kallisto_server"]
