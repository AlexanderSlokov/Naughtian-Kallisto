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
# Base image already provides vcpkg at /usr/local/vcpkg with correct
# permissions for group 'vcpkg' (user vscode is a member).
ENV VCPKG_ROOT=/usr/local/vcpkg
ENV PATH="${VCPKG_ROOT}:${PATH}"

# Pre-install vcpkg dependencies (COPY only manifest files for caching)
COPY vcpkg.json /tmp/vcpkg-manifest/vcpkg.json
RUN cd /tmp/vcpkg-manifest \
    && /usr/local/vcpkg/vcpkg install --triplet x64-linux \
    && rm -rf /tmp/vcpkg-manifest

# ===========================================================================
# Stage 3: Rust toolchain + cargo dependency pre-fetch (as vscode)
# ===========================================================================
USER vscode
WORKDIR /home/vscode

# Install Rust Toolchain
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y \
	&& echo "source \$HOME/.cargo/env" >> $HOME/.bashrc \
	&& . $HOME/.cargo/env && cargo --version && rustc --version

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
