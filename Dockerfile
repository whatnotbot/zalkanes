# syntax=docker/dockerfile:1
FROM rust:1.86-slim-bookworm AS builder

WORKDIR /build

# Install minimal build deps
RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config libssl-dev clang && \
    rm -rf /var/lib/apt/lists/*

# Add wasm target
RUN rustup target add wasm32-unknown-unknown

# Copy workspace
COPY . .

# Build the CLI (release)
RUN cargo build --release -p zalkanes-cli

# ── Runtime image ────────────────────────────────────────────────────────────
FROM debian:bookworm-slim AS runtime

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates libssl3 && \
    rm -rf /var/lib/apt/lists/*

COPY --from=builder /build/target/release/zalkanes /usr/local/bin/zalkanes

ENV RUST_LOG=zalkanes=info

EXPOSE 3030

ENTRYPOINT ["zalkanes"]
CMD ["node", "status"]
