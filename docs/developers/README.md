# Zalkanes developer documentation

Zalkanes is a smart-contract platform anchored to Zcash. You write a contract
in Rust, compile it to `wasm32-unknown-unknown`, publish the bytes on the
Zcash chain, and every Zalkanes node executes it deterministically and agrees
on the resulting state root. Zcash provides ordering and data availability;
Zalkanes provides the deterministic WASM runtime, persistent state, an
indexer, a JSON-RPC server, a CLI, and a contract SDK.

This section is for developers building and deploying contracts. It does not
require reading the consensus internals; those live in `docs/protocol-v0.md`,
`docs/wasm-consensus.md`, and `docs/adr/`.

## Start here

1. [Quickstart](quickstart.md): clone, build, run a local regtest, deploy the
   counter, call it, read it back. About ten commands.
2. [Local regtest guide](local-regtest.md): the same flow step by step, with
   what each command does, how to check the state root, and how to prove
   state survives a node restart.
3. [Contract model](contract-model.md): how a contract executes, its exact
   entrypoint, storage, traps, and fuel.
4. [SDK reference](sdk.md): every function in `zalkanes-sdk`, plus a complete
   counter and a key/value contract.

## Reference

| page | contents |
|---|---|
| [building.md](building.md) | Compiling contracts, the `contract build` command, reproducible flags |
| [deploying.md](deploying.md) | PREPARE then DEPLOY, `ContractId`, immutability, the P2SH carrier |
| [calling-contracts.md](calling-contracts.md) | CALL versus VIEW, input and output encoding, execution records |
| [wallet-funding.md](wallet-funding.md) | Transparent and shielded funding, what shielding does and does not hide |
| [wasm-rules.md](wasm-rules.md) | What a contract must not depend on, host imports, validation |
| [protocol-limits.md](protocol-limits.md) | Every consensus limit, generated from `protocol/v0.toml` |
| [testnet.md](testnet.md) | Public testnet status and procedure |
| [troubleshooting.md](troubleshooting.md) | Errors you will see and what they mean |

## Example contracts

| contract | status on protocol v0 | demonstrates |
|---|---|---|
| [counter](../../contracts/counter/README.md) | supported, used by the quickstart and CI | storage, opcodes, big-endian `u64` output |
| [key-value](../../contracts/key-value/README.md) | supported | variable-length keys and values, delete, failing calls |
| [token](../../contracts/token/README.md) | educational only | a balance ledger; **no access control**, because v0 gives contracts no caller identity |
| [caller](../../contracts/caller/README.md) | **not deployable on v0** | the shape of a nested call and of a deliberate trap; rejected at deploy |

Counter and key-value are the supported developer examples. Token and caller
exist to illustrate specific points and are not templates for applications.

## The CLI in one screen

Every command below exists in `zalkanes` today. Nothing else does.

```text
zalkanes node status                 # read the local state database (node must be stopped)
zalkanes node serve                  # index Zebra blocks and serve JSON-RPC on :3030
zalkanes contract build              # cargo build for wasm32-unknown-unknown
zalkanes contract fund               # regtest only: mine 110 blocks to the dev address
zalkanes contract deploy <wasm>      # PREPARE + DEPLOY via real Zcash transactions
zalkanes contract call <id> <op> [hex]   # CALL via a real Zcash transaction
zalkanes contract view <id> <op> [hex]   # read-only execution over JSON-RPC
zalkanes wallet create|restore|address|balance|status|scan|unlock|lock
zalkanes state-root [height]         # state root from the local database (node stopped)
zalkanes trace <txid>                # placeholder: prints a message and exits
```

`zalkanes <command> --help` prints the flags. Money-moving commands take
`--funding transparent|shielded|auto`, `--dry-run`, `--yes`, `--wait`, and on
mainnet also `--confirm-mainnet`; mainnet execution is disabled in this
release regardless (see [testnet.md](testnet.md)).

## Environment variables the CLI reads

| variable | used by | meaning |
|---|---|---|
| `ZALKANES_NETWORK` | all | `regtest` (default), `testnet`, `mainnet` |
| `ZALKANES_RPC_URL` | `node serve`, `node status` | Zebra JSON-RPC endpoint the indexer reads blocks from |
| `ZALKANES_RPC_API_KEY` | `node serve` | optional API key for that endpoint |
| `ZALKANES_DATA_DIR` | `node serve`, `node status`, `state-root` | RocksDB state directory (default `./zalkanes-data`) |
| `ZALKANES_ZCASH_RPC_URL` | `contract fund/deploy/call`, `wallet *` | Zebra JSON-RPC endpoint used to fund and broadcast |
| `ZALKANES_URL` | `contract view`, `--wait` | Zalkanes JSON-RPC endpoint (default `http://127.0.0.1:3030`) |
| `ZALKANES_SIGNING_KEY` | transparent funding | 32-byte hex secp256k1 key; unset means a fixed dev key, regtest only |
| `ZALKANES_FUNDING_TXID`, `ZALKANES_FUNDING_VOUT` | transparent funding on testnet | the faucet payment to spend |
| `ZALKANES_WALLET_DIR` | `wallet *`, shielded funding | wallet directory (default `~/.zalkanes/wallet/<network>`) |
| `ZALKANES_PASSPHRASE` | `wallet *`, shielded funding | keystore passphrase for non-interactive use |
| `PORT` | `node serve` | JSON-RPC bind port (default 3030; health on `PORT+1`) |
| `RUST_LOG` | all | log filter (default `zalkanes=info`) |

`scripts/run-regtest.sh env` prints the four exports a regtest session needs.

## Network status

Protocol v0 is frozen as release candidate `audit-candidate-v0-rc2`. The
public testnet activation for that candidate is **not yet active**, and
mainnet is **disabled** (`MAINNET_ACTIVATION_HEIGHT = None`). Regtest works
today and is what these docs use. Details: [testnet.md](testnet.md).

## Scope

This repository is the generic Zalkanes platform only. It contains no
decentralized exchange, automated market maker, bridge, wrapped asset,
staking, governance, sequencer, or rollup, and none is planned here.
Applications are built on top of the platform, in their own repositories.
