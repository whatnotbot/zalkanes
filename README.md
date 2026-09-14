# Zalkanes

Zalkanes is a smart-contract platform anchored to Zcash. Contracts are
written in Rust, compiled to WebAssembly, published on the Zcash chain, and
executed deterministically by every Zalkanes node, so any operator running
Zebra plus Zalkanes independently reproduces the same state root. Zcash
provides ordering and data availability; Zalkanes provides the runtime,
state, indexer, RPC, CLI, and SDK.

## Quick start

```bash
cargo build --release
export PATH="$PWD/target/release:$PATH"
./scripts/run-regtest.sh start
eval "$(./scripts/run-regtest.sh env)"
zalkanes contract build --manifest-path ./contracts/counter
zalkanes contract deploy ./contracts/counter/target/wasm32-unknown-unknown/release/counter.wasm --funding transparent --yes --wait
zalkanes contract call "$CONTRACT_ID" 1 "" --funding transparent --yes --wait
zalkanes contract view "$CONTRACT_ID" 2 "" --rpc-url http://127.0.0.1:3030
```

Needs Rust 1.88 (pinned) and `zebrad` v6.3.0 (or Docker). Full walkthrough,
including where `CONTRACT_ID` comes from: [docs/developers/quickstart.md](docs/developers/quickstart.md).
`scripts/dev-quickstart-test.sh` runs the same commands end to end.

## What Zalkanes provides

- Deterministic Rust/WASM smart contracts (`wasmi` 2.0.0, fuel-metered, no floats).
- Zcash ordering and data availability: code and calls live in Zcash transactions.
- Independent replay: any node rebuilds identical state from Zebra alone.
- Persistent state with atomic commits and reorg rollback.
- Permissionless deployment: anyone with ZEC can deploy and call.

## Architecture

```text
Zebra (your own full node)
  ↓  validated blocks over JSON-RPC
Zalkanes (parser · carrier · WASM runtime · state · indexer)
  ↓
RPC / CLI / SDK
```

Crate layout and responsibilities: [docs/architecture.md](docs/architecture.md).

## Developer documentation

[docs/developers/README.md](docs/developers/README.md): quickstart, contract
model, SDK reference, building, deploying, calling, wallet and privacy, WASM
rules, protocol limits, testnet, troubleshooting. Example contracts:
[counter](contracts/counter/README.md), [key-value](contracts/key-value/README.md)
(supported), [token](contracts/token/README.md), [caller](contracts/caller/README.md)
(illustrative only).

## Protocol

[docs/protocol-v0.md](docs/protocol-v0.md) (wire format, frozen),
[docs/wasm-consensus.md](docs/wasm-consensus.md), [protocol/v0.toml](protocol/v0.toml)
(the canonical manifest), [docs/adr/](docs/adr/).

## Operators

[docs/operator-guide.md](docs/operator-guide.md).

## Security / audit

[audit/README.md](audit/README.md), [SECURITY.md](SECURITY.md),
[docs/threat-model/](docs/threat-model/). Protocol v0 has not been externally
audited.

## Network status

- Regtest: works today; used by the docs and CI.
- Testnet: candidate `audit-candidate-v0-rc2`, activation height 4,346,500,
  **not yet active**. See [docs/developers/testnet.md](docs/developers/testnet.md).
- Mainnet: **disabled** (`MAINNET_ACTIVATION_HEIGHT = None`).

## Scope

This repository is the platform only: no exchange, market maker, bridge,
wrapped asset, staking, governance, sequencer, or rollup. See
[CONTRIBUTING.md](CONTRIBUTING.md).

## License

MIT
