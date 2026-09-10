# Zalkanes

Zcash-native WASM smart contracts. Deterministic. Permissionless. Anchored to Zcash consensus.

## What it is

Zalkanes lets anyone deploy and call deterministic WASM smart contracts whose ordering and data availability are anchored to Zcash. Any operator running Zebra + Zalkanes can independently reproduce identical state roots.

```bash
zalkanes contract new counter
zalkanes contract build
zalkanes contract deploy ./counter.wasm
zalkanes contract call <contract-id> increment
zalkanes contract view <contract-id> get
```

## Architecture

```
         Zcash P2P
             │
             ▼
     ┌───────────────┐
     │     Zebra     │   ← consensus trust anchor
     └───────┬───────┘
             │ validated blocks
             ▼
     ┌───────────────┐
     │   Zalkanes    │   ← parser / WASM runtime / state
     └───────┬───────┘
             │ deterministic state root
```

## Status

**v0 — pre-audit, pre-testnet. No mainnet activation height is set.**

See [docs/upstream-lock.md](docs/upstream-lock.md) for pinned dependency versions.

## Prerequisites

- Rust 1.86 (pinned via `rust-toolchain.toml`)
- A locally synced Zebra node (default RPC: `http://127.0.0.1:8232`)
- Docker + Docker Compose (for regtest)

## Quick start (regtest)

```bash
./scripts/run-regtest.sh   # start Zebra regtest + Zalkanes
./scripts/test-e2e.sh      # deploy counter, call increment twice, assert get() == 2
```

## Workspace layout

```
crates/
  zalkanes-core       consensus constants, types
  zalkanes-chain      Zebra RPC chain source
  zalkanes-protocol   canonical parser + serializer
  zalkanes-carrier    P2SH carrier encoding/decoding
  zalkanes-runtime    Wasmi execution profile
  zalkanes-state      RocksDB state engine + state root
  zalkanes-indexer    block indexer + reorg handling
  zalkanes-rpc        JSON-RPC server
  zalkanes-sdk        contract SDK + macros
  zalkanes-build      reproducible WASM build tooling
  zalkanes-tx         transaction construction + fee calc
  zalkanes-testkit    in-memory test harness
  zalkanes-cli        CLI binary
contracts/
  counter  key-value  token  caller
```

## Security model

See [docs/security-model.md](docs/security-model.md) and [docs/threat-model/](docs/threat-model/).

Zalkanes does NOT trust public RPC providers, lightwalletd, or any centralized indexer.
Production deployments must run a local Zebra instance.

## License

MIT
