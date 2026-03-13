# Stage 1: chef — prepare the recipe for dependency caching
# Pin base images to SHA digests to prevent supply-chain attacks via tag mutation.
# To update: docker pull rust:bookworm && docker inspect --format='{{index .RepoDigests 0}}' rust:bookworm
FROM rust:bookworm@sha256:6a544e5d08298a8cddfe9e7d3b4796e746601d933f3b40b3cccc7acdfcd66e0d AS chef
RUN cargo install cargo-chef
WORKDIR /app

COPY . .
RUN cargo chef prepare --recipe-path recipe.json

# Stage 2: builder — cook deps then build the release binary
FROM rust:bookworm@sha256:6a544e5d08298a8cddfe9e7d3b4796e746601d933f3b40b3cccc7acdfcd66e0d AS builder
RUN cargo install cargo-chef
WORKDIR /app

COPY --from=chef /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json

COPY . .
RUN cargo build --release -p gateway

# Stage 3: runtime — minimal image with only what's needed to run
# To update: docker pull debian:bookworm-slim && docker inspect --format='{{index .RepoDigests 0}}' debian:bookworm-slim
FROM debian:bookworm-slim@sha256:74d56e3931e0d5a1dd51f8c8a2466d21de84a271cd3b5a733b803aa91abf4421 AS runtime
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
