# Quickstart

From a clone to a deployed, called, and queried contract on a local regtest.
Every command below is run verbatim by `scripts/dev-quickstart-test.sh`, and
CI fails if the two drift apart.

## Prerequisites

- Rust 1.88.0 with the `wasm32-unknown-unknown` target. `rust-toolchain.toml`
  pins the version and the target; `rustup` installs both on first use.
- A C toolchain (`clang`), needed to build RocksDB.
- `zebrad` v6.3.0 on your `PATH`, **or** Docker with Compose. Zebra publishes
  Linux binaries only; on macOS build it from source. See
  [local-regtest.md](local-regtest.md#getting-zebrad) for both.
- `curl` and `python3` (used by the helper scripts).

## 1. Build Zalkanes

```bash
git clone https://github.com/whatnotbot/zalkanes.git
cd zalkanes
cargo build --release
export PATH="$PWD/target/release:$PATH"
```

## 2. Start Zebra regtest and a Zalkanes node

```bash
./scripts/run-regtest.sh start
eval "$(./scripts/run-regtest.sh env)"
```

The first command starts a private `zebrad` regtest node (RPC on
`127.0.0.1:18232`) and a Zalkanes indexer (JSON-RPC on `127.0.0.1:3030`). The
second exports the environment the CLI needs (`ZALKANES_NETWORK`,
`ZALKANES_ZCASH_RPC_URL`, `ZALKANES_URL`, `ZALKANES_DATA_DIR`).

## 3. Build the counter contract

```bash
zalkanes contract build --manifest-path ./contracts/counter
```

This runs `cargo build --release --target wasm32-unknown-unknown` for the
crate and prints the artifact path:
`./contracts/counter/target/wasm32-unknown-unknown/release/counter.wasm`.

## 4. Fund the regtest wallet

```bash
zalkanes contract fund
```

Regtest only: mines 110 blocks to the deterministic dev address so a mature
coinbase output exists to spend. (Deploy and call also do this themselves on
regtest, so this step is a warm-up; on testnet you fund from a faucet
instead.)

## 5. Deploy

```bash
zalkanes contract deploy ./contracts/counter/target/wasm32-unknown-unknown/release/counter.wasm --funding transparent --yes --wait
```

Two real Zcash transactions are built, signed, broadcast, and mined: PREPARE
(creates the carrier outputs) and DEPLOY (spends them, publishing the WASM).
The last lines look like:

```text
DEPLOY mined at height 222: 99db762091a5ae5642fd5fb1ce00d45be1a44c64d7cf4b556a425445bfeee0ea
ContractId: 2d7e818e524195f15ee513dc5fa3c58999e99f4e2e35a765a244a6f4526e237f
indexed: {"code_hash":"fa8289fb…","code_size":2513,"contract_id":"2d7e818e…"}
```

Copy the `ContractId` value:

```bash
CONTRACT_ID=<paste the 64-hex ContractId printed by deploy>
```

## 6. Call it twice

Opcode `1` is `increment`. The empty string is the (empty) input.

```bash
zalkanes contract call "$CONTRACT_ID" 1 "" --funding transparent --yes --wait
zalkanes contract call "$CONTRACT_ID" 1 "" --funding transparent --yes --wait
```

Each call is one Zcash transaction. `--wait` blocks until the Zalkanes node
has indexed the execution and prints the execution record (`"success":true`,
`"fuel_used":3936`).

## 7. Read it back

Opcode `2` is `get`; it returns the counter as a big-endian `u64`.

```bash
zalkanes contract view "$CONTRACT_ID" 2 "" --rpc-url http://127.0.0.1:3030
```

```json
{"jsonrpc":"2.0","id":1,"result":{"success":true,"output_hex":"0000000000000002","fuel_used":3936,"indexed_height":444,"state_root":"fb6b7f36…","error":null}}
```

`output_hex` is `2`. A view executes the contract locally against the node's
indexed state and broadcasts nothing.

## 8. Stop

```bash
./scripts/run-regtest.sh stop
```

## Next

- Why heights jump by about 111 per command, how to check the state root,
  and how to prove persistence: [local-regtest.md](local-regtest.md).
- What the contract you just deployed actually does: [contract-model.md](contract-model.md)
  and [contracts/counter/README.md](../../contracts/counter/README.md).
- Writing your own: [sdk.md](sdk.md), [building.md](building.md).
