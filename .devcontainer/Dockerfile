FROM mcr.microsoft.com/devcontainers/cpp:ubuntu-24.04

RUN curl -fsSL https://dl.k6.io/key.gpg | sudo gpg --dearmor -o /usr/share/keyrings/k6-archive-keyring.gpg

RUN echo "deb [signed-by=/usr/share/keyrings/k6-archive-keyring.gpg] https://dl.k6.io/deb stable main" | sudo tee /etc/apt/sources.list.d/k6.list

RUN apt-get update && \
    apt-get upgrade -y && \
    apt-get install -y \
    build-essential \
    clang \
    git \
    python3 \
    wget \
    curl \
    procps \
    sudo \
    ca-certificates \
	k6 \
    && apt-get clean && rm -rf /var/lib/apt/lists/*

# Rust toolchain + cargo dependency pre-fetch
# -------------------------------------------
USER vscode
WORKDIR /home/vscode

# Install Rust Toolchain
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y \
	&& echo "source \$HOME/.cargo/env" >> $HOME/.bashrc \
	&& . $HOME/.cargo/env && cargo --version && rustc --version

# Make Rust toolchain visible to ALL users — this ensures rustc is found whether or not .bashrc was sourced.
ENV PATH="/home/vscode/.cargo/bin:${PATH}"

# Pre-fetch Rust dependencies using skeleton crate technique:
# Copy only Cargo.toml/Cargo.lock files, create dummy src/ files, then cargo fetch.
# This layer is cached as long as Cargo.toml/Cargo.lock don't change.
COPY --chown=vscode:vscode --chmod=755 Cargo.toml Cargo.loc[k] /tmp/rust-deps/
COPY --chown=vscode:vscode --chmod=755 cmd/kallisto-ctl/Cargo.toml /tmp/rust-deps/cmd/kallisto-ctl/
COPY --chown=vscode:vscode --chmod=755 cmd/kallisto-server/Cargo.toml /tmp/rust-deps/cmd/kallisto-server/
COPY --chown=vscode:vscode --chmod=755 components/kallisto_cluster/Cargo.toml /tmp/rust-deps/components/kallisto_cluster/
COPY --chown=vscode:vscode --chmod=755 components/kallisto_crypto/Cargo.toml /tmp/rust-deps/components/kallisto_crypto/
COPY --chown=vscode:vscode --chmod=755 components/kallisto_policy/Cargo.toml /tmp/rust-deps/components/kallisto_policy/
COPY --chown=vscode:vscode --chmod=755 components/kallisto_telemetry/Cargo.toml /tmp/rust-deps/components/kallisto_telemetry/

# Create dummy source files so cargo can resolve the workspace
RUN bash -c 'source $HOME/.cargo/env \
    && for crate in cmd/kallisto-ctl cmd/kallisto-server components/kallisto_cluster components/kallisto_crypto components/kallisto_policy components/kallisto_telemetry; do \
         mkdir -p /tmp/rust-deps/$crate/src; \
         echo "// placeholder" > /tmp/rust-deps/$crate/src/lib.rs; \
         echo "fn main() {}" > /tmp/rust-deps/$crate/src/main.rs; \
       done \
    && cd /tmp/rust-deps \
    && cargo fetch \
    && rm -rf /tmp/rust-deps'

# Dev tools
# ---------

# wrk2
RUN git clone https://github.com/giltene/wrk2.git wrk && \
	cd wrk && \
	sudo make && \
	sudo cp wrk /usr/local/bin