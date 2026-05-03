FROM mcr.microsoft.com/devcontainers/cpp:ubuntu-24.04

# 1. Cài thêm 'sudo' và các gói cần thiết
RUN apt-get update && apt-get install -y \
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

USER vscode

WORKDIR /home/vscode

# Install Rust Toolchain
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y \
	&& echo "source \$HOME/.cargo/env" >> $HOME/.bashrc \
	&& source $HOME/.cargo/env && cargo --version && rustc --version

# Install vcpkg
RUN git clone https://github.com/microsoft/vcpkg.git "$HOME/vcpkg" && cd "$HOME/vcpkg" && ./bootstrap-vcpkg.sh
RUN export VCPKG_ROOT="$HOME/vcpkg" && export PATH="$VCPKG_ROOT:$PATH"

# Install wrk
RUN git clone https://github.com/wg/wrk.git wrk && cd wrk && sudo make && sudo cp wrk /usr/local/bin
