# syntax=docker/dockerfile:1.6
# Zebvix Chain — multi-stage release image
#
# Build:   docker build -t zbx/zbx-node:0.2.0 -f zbx-chain/docker/Dockerfile .
# Run:     docker run -d --name zbx-mainnet \
#              -p 8545:8545 -p 8546:8546 -p 30303:30303 \
#              -v zbx-mainnet-data:/var/lib/zbx-mainnet \
#              zbx/zbx-node:0.2.0 --network mainnet --bind-addr 0.0.0.0

# ---------- Builder ----------
FROM rust:1.82-bookworm AS builder
WORKDIR /workspace

# Install RocksDB build deps
RUN apt-get update && apt-get install -y --no-install-recommends \
        clang libclang-dev cmake pkg-config libssl-dev \
    && rm -rf /var/lib/apt/lists/*

# Cache deps via a stub workspace first
COPY zbx-chain/Cargo.toml zbx-chain/Cargo.lock ./zbx-chain/
COPY zbx-chain/crates ./zbx-chain/crates
COPY zbx-chain/node   ./zbx-chain/node

WORKDIR /workspace/zbx-chain
ENV CARGO_NET_RETRY=10 \
    CARGO_BUILD_JOBS=4 \
    RUSTFLAGS="-C target-cpu=x86-64-v3"
RUN cargo build --release -p zbx-node --bin zbx-node --bin zbx-keygen

# ---------- Runtime ----------
FROM debian:bookworm-slim AS runtime
RUN apt-get update && apt-get install -y --no-install-recommends \
        ca-certificates libssl3 curl \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --system --gid 1001 zbx \
    && useradd  --system --uid 1001 --gid zbx --no-create-home --shell /usr/sbin/nologin zbx

COPY --from=builder /workspace/zbx-chain/target/release/zbx-node   /usr/local/bin/zbx-node
COPY --from=builder /workspace/zbx-chain/target/release/zbx-keygen /usr/local/bin/zbx-keygen
COPY zbx-chain/node/configs/mainnet.toml /etc/zbx/mainnet.toml
COPY zbx-chain/node/configs/testnet.toml /etc/zbx/testnet.toml

RUN mkdir -p /var/lib/zbx-mainnet /var/lib/zbx-testnet \
    && chown -R zbx:zbx /var/lib/zbx-mainnet /var/lib/zbx-testnet /etc/zbx

USER zbx
WORKDIR /var/lib/zbx-mainnet
EXPOSE 8545 8546 18545 18546 30303 30304 9000 9001

HEALTHCHECK --interval=30s --timeout=5s --start-period=15s --retries=3 \
    CMD curl -fsS -X POST -H 'Content-Type: application/json' \
        --data '{"jsonrpc":"2.0","id":1,"method":"eth_chainId","params":[]}' \
        http://127.0.0.1:8545/ || exit 1

ENTRYPOINT ["/usr/local/bin/zbx-node"]
CMD ["--network", "mainnet", "--config", "/etc/zbx/mainnet.toml", "--bind-addr", "0.0.0.0"]
