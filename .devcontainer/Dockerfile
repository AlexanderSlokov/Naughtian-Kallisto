FROM mcr.microsoft.com/devcontainers/cpp:ubuntu-24.04

# ===========================================================================
# Stage 1: System packages + Python tools (as root)
# ===========================================================================
RUN apt-get update && \
    apt-get upgrade -y && \
    apt-get install -y \
    build-essential \
    clang \
    cmake \
    gdb \
    git \
    python3 \
    python3-pip \
    python3-venv \
    valgrind \
    wget \
    curl \
    procps \
    sudo \
    ca-certificates \
    libspdlog-dev \
    && apt-get clean && rm -rf /var/lib/apt/lists/*

# Install gcovr (moved from postCreateCommand for layer caching)
RUN pip install gcovr --break-system-packages

# ===========================================================================
# Stage 2: Pre-install vcpkg C++ dependencies (reuse vcpkg from base image)
# ===========================================================================
# Base image provides vcpkg at /usr/local/vcpkg. User vscode is in group
# 'vcpkg', but `vcpkg install` runs as root here (for Docker layer caching).
# We use a shared binary cache so vscode can reuse pre-built packages at
# runtime without recompiling from source.
ENV VCPKG_ROOT=/usr/local/vcpkg
ENV PATH="${VCPKG_ROOT}:${PATH}"

# Shared binary cache: both root (docker build) and vscode (runtime) use
# the same location. Without this, root caches to /root/.cache/ and vscode
# can't find the pre-built packages → full recompile at first cmake run.
ENV VCPKG_DEFAULT_BINARY_CACHE=/usr/local/vcpkg/archives
RUN mkdir -p /usr/local/vcpkg/archives \
    && chown root:vcpkg /usr/local/vcpkg/archives \
    && chmod 2775 /usr/local/vcpkg/archives

# Pre-install vcpkg dependencies (COPY only manifest files for caching)
COPY vcpkg.json /tmp/vcpkg-manifest/vcpkg.json
RUN cd /tmp/vcpkg-manifest \
    && /usr/local/vcpkg/vcpkg install --triplet x64-linux \
    && rm -rf /tmp/vcpkg-manifest

# Fix vcpkg permissions for runtime use by vscode (group vcpkg):
# - buildtrees/: vcpkg writes compiler detection results here
# - packages/:   vcpkg stages package files here before installing
# - downloads/:  vcpkg may download new source tarballs here
# Root's umask (022) created these dirs without group-write. Fix that.
RUN chmod -R g+w /usr/local/vcpkg/buildtrees \
                  /usr/local/vcpkg/packages \
                  /usr/local/vcpkg/downloads 2>/dev/null; \
    find /usr/local/vcpkg/buildtrees \
         /usr/local/vcpkg/packages \
         /usr/local/vcpkg/downloads \
         -type d -exec chmod g+ws {} + 2>/dev/null; \
    exit 0

# ===========================================================================
# Stage 3: Rust toolchain + cargo dependency pre-fetch (as vscode)
# ===========================================================================
USER vscode
WORKDIR /home/vscode

# Install Rust Toolchain
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y \
	&& echo "source \$HOME/.cargo/env" >> $HOME/.bashrc \
	&& . $HOME/.cargo/env && cargo --version && rustc --version

# Make Rust toolchain visible to ALL users (including cmake running without
# an interactive shell). Corrosion's FindRust.cmake checks PATH — this
# ensures rustc is found whether or not .bashrc was sourced.
ENV PATH="/home/vscode/.cargo/bin:${PATH}"

# Pre-fetch Rust dependencies using skeleton crate technique:
# Copy only Cargo.toml/Cargo.lock files, create dummy lib.rs, then cargo fetch.
# This layer is cached as long as Cargo.toml/Cargo.lock don't change.
COPY --chown=vscode:vscode --chmod=755 rust_integrates/Cargo.toml /tmp/rust-deps/Cargo.toml
COPY --chown=vscode:vscode --chmod=755 rust_integrates/Cargo.lock       /tmp/rust-deps/Cargo.lock
COPY --chown=vscode:vscode --chmod=755 rust_integrates/ffi_bridge/Cargo.toml     /tmp/rust-deps/ffi_bridge/Cargo.toml
COPY --chown=vscode:vscode --chmod=755 rust_integrates/core_crypto/Cargo.toml    /tmp/rust-deps/core_crypto/Cargo.toml
COPY --chown=vscode:vscode --chmod=755 rust_integrates/telemetry/Cargo.toml      /tmp/rust-deps/telemetry/Cargo.toml
COPY --chown=vscode:vscode --chmod=755 rust_integrates/control_plane/Cargo.toml  /tmp/rust-deps/control_plane/Cargo.toml
COPY --chown=vscode:vscode --chmod=755 rust_integrates/policy_engine/Cargo.toml  /tmp/rust-deps/policy_engine/Cargo.toml
COPY --chown=vscode:vscode --chmod=755 rust_integrates/kallisto_tui/Cargo.toml   /tmp/rust-deps/kallisto_tui/Cargo.toml

# Create dummy source files so cargo can resolve the workspace
RUN bash -c 'source $HOME/.cargo/env \
    && for crate in ffi_bridge core_crypto telemetry control_plane policy_engine kallisto_tui; do \
         mkdir -p /tmp/rust-deps/$crate/src; \
         echo "// placeholder" > /tmp/rust-deps/$crate/src/lib.rs; \
       done \
    && cd /tmp/rust-deps \
    && cargo fetch \
    && rm -rf /tmp/rust-deps'

# ===========================================================================
# Stage 4: Dev tools (wrk benchmark tool)
# ===========================================================================
# Install wrk
RUN git clone https://github.com/wg/wrk.git wrk && cd wrk && sudo make && sudo cp wrk /usr/local/bin
