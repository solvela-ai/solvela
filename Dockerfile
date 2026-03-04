# Stage 1: chef — prepare the recipe for dependency caching
FROM rust:bookworm AS chef
RUN cargo install cargo-chef
WORKDIR /app

COPY . .
RUN cargo chef prepare --recipe-path recipe.json

# Stage 2: builder — cook deps then build the release binary
FROM rust:bookworm AS builder
RUN cargo install cargo-chef
WORKDIR /app

COPY --from=chef /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json

COPY . .
RUN cargo build --release -p gateway

# Stage 3: runtime — minimal image with only what's needed to run
FROM debian:bookworm-slim AS runtime
RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl3 \
    libpq5 \
    && rm -rf /var/lib/apt/lists/*

RUN useradd --uid 1001 --no-create-home --shell /bin/false rcr

COPY --from=builder /app/target/release/rustyclawrouter /usr/local/bin/rustyclawrouter

EXPOSE 8402
USER rcr
CMD ["/usr/local/bin/rustyclawrouter"]
