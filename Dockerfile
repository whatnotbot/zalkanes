# syntax=docker/dockerfile:1
FROM rust:1.88-slim-bookworm AS builder

WORKDIR /build

# Install build deps (clang + libclang for RocksDB, pkg-config, libssl)
RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config libssl-dev clang libclang-dev libsnappy-dev && \
    rm -rf /var/lib/apt/lists/*

# Copy workspace manifests first for dependency caching
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY contracts ./contracts
COPY protocol ./protocol

# Build the CLI (release). Excludes wasm32 contract crates via default-members.
RUN cargo build --release -p zalkanes-cli

# ── Runtime image ────────────────────────────────────────────────────────────
FROM debian:bookworm-slim AS runtime

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates libssl3 libsnappy1v5 && \
    rm -rf /var/lib/apt/lists/*

COPY --from=builder /build/target/release/zalkanes /usr/local/bin/zalkanes

ENV RUST_LOG=zalkanes=info
ENV ZALKANES_NETWORK=regtest

EXPOSE 3030

ENTRYPOINT ["zalkanes"]
CMD ["node", "serve"]
