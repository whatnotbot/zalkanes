# Local regtest guide

An end-to-end tutorial on a private Zcash regtest. Nothing here touches a
public network. Every command was run against `zebrad` v6.3.0 and the CLI in
this repository; the acceptance script `scripts/dev-quickstart-test.sh`
replays the core of it.

## 1. Prerequisites

| requirement | notes |
|---|---|
| Rust 1.88.0 + `wasm32-unknown-unknown` | pinned by `rust-toolchain.toml`; `rustup` installs both |
| `clang` | RocksDB is built from source |
| `zebrad` v6.3.0 | see "Getting zebrad" below |
| `curl`, `python3` | used by `scripts/run-regtest.sh` |

### Getting zebrad

Zalkanes pins Zebra **v6.3.0**. The launcher looks for `zebrad` on `PATH`
(or at `$ZEBRAD`).

- **Linux**: download the release tarball from
  `https://github.com/ZcashFoundation/zebra/releases/tag/v6.3.0`
  (`zebrad-6.3.0-x86_64-unknown-linux-gnu.tar.gz` or the `aarch64` one),
  verify the `.sha256` file, and put `zebrad` on your `PATH`. This is exactly
  what CI does.
- **macOS**: Zebra ships no macOS binaries. Build it:

  ```bash
  git clone --depth 1 --branch v6.3.0 https://github.com/ZcashFoundation/zebra.git
  cd zebra && cargo build --release --locked --bin zebrad
  export PATH="$PWD/target/release:$PATH"
  ```

  Zebra's own `rust-toolchain.toml` selects the Rust version it needs, and the
  build needs `protoc` on `PATH`. On an Apple M-series machine the build takes
  a few minutes.
- **Docker**: if no `zebrad` binary is found but `docker compose` works, the
  launcher falls back to `docker-compose.yml` (pinned `zfnd/zebra:v6.3.0`
  image plus a Zalkanes container). The Docker path is provided as a
  convenience and is not the path the acceptance script exercises.

## 2. Build

```bash
cargo build --release
export PATH="$PWD/target/release:$PATH"
```

## 3. Start Zebra

```bash
./scripts/run-regtest.sh start
```

In native mode this writes `.regtest/zebrad.toml` (regtest with every network
upgrade active from height 1, no peers, RPC on `127.0.0.1:18232` without
cookie auth) and starts `zebrad`. The directory `.regtest/` is git-ignored;
set `ZALKANES_REGTEST_DIR` to move it and `ZALKANES_REGTEST_FRESH=1` to wipe
it before starting.

## 4. Start Zalkanes

The same command also starts the indexer:

```text
ZALKANES_NETWORK=regtest ZALKANES_RPC_URL=http://127.0.0.1:18232 \
ZALKANES_DATA_DIR=.regtest/zalkanes-data zalkanes node serve --port 3030
```

It polls Zebra every three seconds, executes every Zalkanes message in each
new block, and serves JSON-RPC on `127.0.0.1:3030` (health on `3031`). Logs
are in `.regtest/zalkanes.log`. Then load the client environment:

```bash
eval "$(./scripts/run-regtest.sh env)"
```

## 5. Fund the wallet

```bash
zalkanes contract fund
```

```text
WARNING: using deterministic dev signing key (regtest only)
mined 110 blocks to tmYjAZFpvdDXTaJrq2WikAntitBNJJo9VSo
```

On regtest the CLI funds itself: without `ZALKANES_SIGNING_KEY` it uses a
fixed development key, and every transparent deploy or call first mines 110
blocks to that key's address to obtain a mature coinbase output. That is why
block heights advance by about 111 per command. The explicit `fund` step is
optional but shows you the address.

## 6. Build a contract

```bash
zalkanes contract build --manifest-path ./contracts/counter
```

```text
Built ./contracts/counter for wasm32-unknown-unknown
wasm: ./contracts/counter/target/wasm32-unknown-unknown/release/counter.wasm (2513 bytes)
```

## 7. Deploy and capture the ContractId

```bash
zalkanes contract deploy ./contracts/counter/target/wasm32-unknown-unknown/release/counter.wasm --funding transparent --yes --wait
```

The output shows both stages. PREPARE creates one carrier output per 1400-byte
chunk (2 for the counter); DEPLOY spends them and carries the DEPLOY message
in an OP_RETURN. The `ContractId:` line is what you call later.

```text
wasm: 2513 bytes, 2 chunk(s), code hash fa8289fbc0fdb132e57f939033a9971b0ee2990ec49db247aad3f682aa804cc7
deploy fee: 95000 zat; carriers: 2 x 72500 zat
── PREPARE ──
…
PREPARE mined: a3c8603f…
── DEPLOY (transparent carrier spends) ──
…
DEPLOY mined at height 222: 99db7620…
ContractId: 2d7e818e524195f15ee513dc5fa3c58999e99f4e2e35a765a244a6f4526e237f
indexed: {"code_hash":"fa8289fb…","code_size":2513,"contract_id":"2d7e818e…"}
```

```bash
CONTRACT_ID=<paste the 64-hex ContractId printed by deploy>
```

Without `--yes` the CLI prints the funding plan and asks `Proceed? [y/N]`.
`--dry-run` prints the plan and stops before signing.

## 8. CALL

```bash
zalkanes contract call "$CONTRACT_ID" 1 "" --funding transparent --yes --wait
```

A CALL is a real Zcash transaction: an OP_RETURN with the ZALK message
(contract id, opcode, input) and a P2PKH change output, fee per ZIP-317
(15,000 zat for this one). `--wait` polls the node until the execution record
exists:

```text
ZALK payload: 5a414c4b00022d7e818e…00010000
broadcast accepted: 4503507f…
CALL mined at height 333: 4503507f…
execution: {"block_height":333,"success":true,"fuel_used":3936,"error":null,…}
```

Run it a second time to reach 2.

## 9. VIEW

```bash
zalkanes contract view "$CONTRACT_ID" 2 "" --rpc-url http://127.0.0.1:3030
```

```json
{"jsonrpc":"2.0","id":1,"result":{"success":true,"output_hex":"0000000000000002","fuel_used":3936,"indexed_height":444,"state_root":"fb6b7f3644fd6ee4d12f692d62fba024f048b41225d5230f8e2f78362c183142","error":null}}
```

A view runs the same WASM with the same fuel limit against the node's current
indexed state, discards any writes, and returns the output. No transaction.

## 10. Check the state root

While the node is running, ask it over JSON-RPC:

```bash
curl -s -X POST http://127.0.0.1:3030 -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"zalkanes_getInfo","params":[]}'
```

```json
{"jsonrpc":"2.0","id":1,"result":{"protocol_version":0,"protocol_manifest_hash":"06e3df62…","network":"regtest","indexed_height":444,"chain_tip_height":444,"indexed_block_hash":"ab933d84…","state_root":"fb6b7f36…","syncing":false}}
```

`zalkanes_getStateRoot` returns just the root. The root commits to every
deployed contract's code and every storage entry, nothing else. Note the
value; the next two steps must reproduce it.

`zalkanes state-root` and `zalkanes node status` read the RocksDB directory
directly and therefore only work when the node is **stopped** (otherwise:
`failed to open RocksDB … (lock held too long)`).

## 11. Restart the node

```bash
./scripts/run-regtest.sh restart-node
```

This stops and restarts only the Zalkanes indexer; Zebra keeps running.

## 12. Confirm the state persisted

```bash
curl -s -X POST http://127.0.0.1:3030 -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"zalkanes_getStateRoot","params":[]}'
zalkanes contract view "$CONTRACT_ID" 2 "" --rpc-url http://127.0.0.1:3030
```

The root and the view result are identical to step 10. Commits are atomic,
so a restart resumes from the last complete block.

A stronger check is a **clean reindex**: discard the database and let the
node rebuild it from Zebra alone.

```bash
./scripts/run-regtest.sh restart-node --clean
```

Once `indexed_height` reaches the tip again, `zalkanes_getStateRoot` returns
the same root. Two nodes that index the same chain always agree on the root;
that is the property the whole system is built on.

Finally, with the node stopped, the offline reader sees the same value:

```bash
./scripts/run-regtest.sh stop
zalkanes state-root
```

## Timing and heights

| step | wall clock on an Apple M-series laptop | blocks mined |
|---|---|---|
| `contract fund` | ~9 s | 110 |
| `contract deploy` (2 chunks) | ~15 s | 110 + 2 |
| `contract call` | ~8 s | 110 + 1 |

Heights of 222, 333, 444 in the examples above are this pattern, not a
coincidence.

## Cleanup

`./scripts/run-regtest.sh stop` stops both processes. Delete `.regtest/` to
remove the chain and the state. Wallet files (if you created a wallet) live
under `ZALKANES_WALLET_DIR`, default `~/.zalkanes/wallet/regtest`.
