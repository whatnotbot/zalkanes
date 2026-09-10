# syntax=docker/dockerfile:1
# Zebra regtest node with the internal miner, for the Zalkanes private testnet.
FROM rust:1.88-slim-bookworm AS builder

RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config libssl-dev clang libclang-dev protobuf-compiler && \
    rm -rf /var/lib/apt/lists/*

# Pin the Zebra release that Zalkanes locks against.
ARG ZEBRA_TAG=v6.3.0
RUN git clone --depth 1 --branch ${ZEBRA_TAG} https://github.com/ZcashFoundation/zebra.git /zebra

WORKDIR /zebra
# Build zebrad with the experimental internal miner for regtest block production.
RUN cargo build --release --locked --features internal-miner --bin zebrad

# ── Runtime ───────────────────────────────────────────────────────────────────
FROM debian:bookworm-slim AS runtime

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates libssl3 && \
    rm -rf /var/lib/apt/lists/*

COPY --from=builder /zebra/target/release/zebrad /usr/local/bin/zebrad
COPY deploy/zebra/zebra-regtest.toml /etc/zebra/zebra.toml

EXPOSE 18232 18233

ENTRYPOINT ["zebrad"]
CMD ["-c", "/etc/zebra/zebra.toml"]
