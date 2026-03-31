# Stage 1: Build
FROM rust:1.88-slim-bookworm AS builder

RUN apt-get update && apt-get install -y pkg-config libssl-dev && rm -rf /var/lib/apt/lists/*

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

RUN cargo build --release --bin rustyclawrouter 2>/dev/null || true

# Copy actual source code
COPY crates/ crates/
COPY config/ config/

# Touch source files to invalidate the cache for actual compilation
RUN touch crates/protocol/src/lib.rs crates/x402/src/lib.rs crates/router/src/lib.rs crates/gateway/src/lib.rs crates/gateway/src/main.rs

RUN cargo build --release --bin rustyclawrouter

# Stage 2: Runtime
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY --from=builder /app/target/release/rustyclawrouter .
COPY --from=builder /app/config/ config/

EXPOSE 8402

CMD ["./rustyclawrouter"]
