# Stage 1: Build
FROM rust:1.96-slim-trixie AS builder

# g++ is required so the linker can resolve -lstdc++ — the fastembed
# dep pulls in the ONNX Runtime C++ bindings (via `ort` / `ort-sys`),
# which the final link step needs to satisfy.
RUN apt-get update && apt-get install -y pkg-config libssl-dev g++ && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Copy workspace manifests first for dependency caching
COPY Cargo.toml Cargo.lock ./
COPY crates/protocol/Cargo.toml crates/protocol/Cargo.toml
COPY crates/x402/Cargo.toml crates/x402/Cargo.toml
COPY crates/router/Cargo.toml crates/router/Cargo.toml
COPY crates/gateway/Cargo.toml crates/gateway/Cargo.toml
COPY crates/cli/Cargo.toml crates/cli/Cargo.toml

# Create dummy source files to cache dependency compilation
RUN mkdir -p crates/protocol/src crates/x402/src crates/router/src crates/gateway/src crates/cli/src && \
    echo "pub fn _dummy() {}" > crates/protocol/src/lib.rs && \
    echo "pub fn _dummy() {}" > crates/x402/src/lib.rs && \
    echo "pub fn _dummy() {}" > crates/router/src/lib.rs && \
    echo "pub fn _dummy() {}" > crates/gateway/src/lib.rs && \
    echo "fn main() {}" > crates/gateway/src/main.rs && \
    echo "pub fn _dummy() {}" > crates/cli/src/lib.rs && \
    echo "fn main() {}" > crates/cli/src/main.rs

RUN cargo build --release --bin solvela-gateway 2>/dev/null || true

# Copy actual source code
COPY crates/ crates/
COPY config/ config/
# Migration SQL files are read at compile time by `sqlx::migrate!("../../migrations")`
# in crates/gateway/src/main.rs; must be present in the build context.
COPY migrations/ migrations/

# Touch source files to invalidate the cache for actual compilation
RUN touch crates/protocol/src/lib.rs crates/x402/src/lib.rs crates/router/src/lib.rs crates/gateway/src/lib.rs crates/gateway/src/main.rs

RUN cargo build --release --bin solvela-gateway

# Stage 2: Runtime
FROM debian:trixie-slim

# libstdc++6 is the runtime counterpart to the builder's g++ — fastembed's
# ONNX Runtime layer dynamically links against libstdc++.so.6.
RUN apt-get update && apt-get install -y ca-certificates libstdc++6 && rm -rf /var/lib/apt/lists/*

# Create non-root runtime user (no home, no login shell)
RUN useradd --uid 1001 --no-create-home --shell /usr/sbin/nologin solvela

WORKDIR /app

COPY --from=builder --chown=solvela:solvela /app/target/release/solvela-gateway .
COPY --from=builder --chown=solvela:solvela /app/config/ config/

USER solvela

EXPOSE 8402

CMD ["./solvela-gateway"]
