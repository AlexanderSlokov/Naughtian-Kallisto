# syntax=docker/dockerfile:1

# ==============================================================================
# Phase 1: Build Image (Ubuntu 24.04)
# ==============================================================================
FROM ubuntu:24.04 AS builder

ENV DEBIAN_FRONTEND=noninteractive

# Install dependencies needed for vcpkg and C++ build
RUN apt-get update && apt-get install -y --no-install-recommends \
    build-essential \
    ca-certificates \
    cmake \
    curl \
    git \
    ninja-build \
    pkg-config \
    python3 \
    tar \
    unzip \
    zip \
    && rm -rf /var/lib/apt/lists/*

# Install vcpkg
ENV VCPKG_ROOT=/usr/local/vcpkg
RUN git clone https://github.com/microsoft/vcpkg.git "$VCPKG_ROOT" \
    && "$VCPKG_ROOT"/bootstrap-vcpkg.sh -disableMetrics

WORKDIR /app
COPY . .

# Configure and build all binaries using the custom triplet to avoid building debug
RUN mkdir -p build && cd build \
    && cmake -DCMAKE_TOOLCHAIN_FILE="$VCPKG_ROOT"/scripts/buildsystems/vcpkg.cmake \
             -DCMAKE_BUILD_TYPE=Release \
             -DVCPKG_TARGET_TRIPLET=x64-linux-release \
             -DVCPKG_HOST_TRIPLET=x64-linux-release \
             -DVCPKG_OVERLAY_TRIPLETS=/app/custom-triplets \
             .. \
    && make -j$(nproc)

# ==============================================================================
# Phase 2: Test Image (Specialized image containing libraries to run tests)
# ==============================================================================
FROM ubuntu:24.04 AS tester

ENV DEBIAN_FRONTEND=noninteractive

# Install dependencies for running tests and benchmarks (make, bash, python, wrk, curl)
RUN apt-get update && apt-get install -y --no-install-recommends \
    bash \
    ca-certificates \
    curl \
    iproute2 \
    make \
    procps \
    python3 \
    python3-pip \
    wrk \
    && rm -rf /var/lib/apt/lists/* \
    && pip3 install --only-binary :all: gcovr==8.6 --break-system-packages

# Add non-root user for running tests securely
RUN useradd -m -s /bin/bash kallisto \
    && mkdir -p /kallisto/data \
    && mkdir -p /var/run/kallisto \
    && chown -R kallisto:kallisto /kallisto \
    && chown -R kallisto:kallisto /var/run/kallisto

WORKDIR /app

# Copy the entire build context and compiled binaries from the builder
COPY --from=builder /app /app

# Change ownership to kallisto user
RUN chown -R kallisto:kallisto /app
USER kallisto

# By default, running this stage executes unit tests
CMD ["make", "test"]

# ==============================================================================
# Phase 3: Production Image
# Using 'noble' (24.04) to match dev environment
# ==============================================================================
FROM ubuntu:24.04 AS production

ENV DEBIAN_FRONTEND=noninteractive

# Create a non-root user to run the software
RUN groupadd kallisto \
    && useradd -r -g kallisto -s /bin/bash kallisto

# Install runtime dependencies
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    dumb-init \
    gosu \
    libcap2-bin \
    tzdata \
    && rm -rf /var/lib/apt/lists/*

# Prepare log, config, data storage backend and IPC socket directory
RUN mkdir -p /kallisto/logs \
    && mkdir -p /kallisto/config \
    && mkdir -p /kallisto/data \
    && mkdir -p /var/run/kallisto \
    && chown -R kallisto:kallisto /kallisto \
    && chown -R kallisto:kallisto /var/run/kallisto

# Expose the logs directory as a volume since there's potentially long-running
# state in there
VOLUME ["/kallisto/logs"]

# Expose the persistent data directory as a volume since there's potentially
# long-running state in there
VOLUME ["/kallisto/data"]

WORKDIR /app

# Copy the server executable from the builder stage
COPY --from=builder /app/build/kallisto_server /app/kallisto_server
COPY docker/entrypoint.sh /app/entrypoint.sh

# Guarantee script is executable and owned by Kallisto
RUN chmod +x /app/entrypoint.sh \
    && chown kallisto:kallisto /app/kallisto_server /app/entrypoint.sh

# Use the kallisto user as the default user for starting this container
USER kallisto

# 8200/tcp is the primary interface that applications use to interact with Kallisto
EXPOSE 8200

ENTRYPOINT ["/app/entrypoint.sh"]
