# Public testnet

Status is read from the repository's canonical release data
(`protocol/v0.toml`, `audit/gate-attestations.json`) and enforced by
`scripts/check-docs.sh`: the line below may only say `ACTIVE` once the
activation height has been reached **and** the one-time platform acceptance
(`docs/release/testnet-rc2-deployment.md` §5 and §6) has passed and been
recorded in `audit/gate-attestations.json`.

```text
Network status:    NOT YET ACTIVE
protocol:          v0
candidate:         audit-candidate-v0-rc2
activation height: 4,346,500
manifest hash:     06e3df62e5e98a3c276b583e038cbea8d05934e9d2a8e7c00299fec3140bf4bb
mainnet:           disabled
```

> **v0 limitations you must design around**
>
> - **No caller identity.** A contract cannot learn who called it. Anything
>   that looks like authorization must come from the input bytes, which anyone
>   can supply.
> - **No cross-contract calls.** One CALL executes one contract.
> - **No events.** The only outputs are storage writes and return data.
> - **CLI inline CALL input is at most 38 bytes.** Larger inputs need a
>   carrier CALL, which the CLI does not build yet. Views have no such limit.
> - **Contract code, calldata, and state are public.** Shielded funding hides
>   only the source of funds, never the contract interaction.
> - **No Zalkanes gas market.** Fuel is a hard per-call cap; the only cost is
>   the Zcash transaction fee (ZIP-317).
>
> These are properties of protocol v0, not bugs; see
> [contract-model.md](contract-model.md) and [wasm-rules.md](wasm-rules.md).

## Supported testnet RPC endpoint

Two independently initialised Zalkanes nodes follow the project's own Zebra
testnet validator and are checked for agreement on height, block hash, and
state root. Use node A; node B exists so you can verify independence.

| role | JSON-RPC URL |
|---|---|
| **node A (supported endpoint)** | `https://zalkanes-testnet-rc2-a-production.up.railway.app` |
| node B (independent comparison) | `https://zalkanes-testnet-rc2-b-production.up.railway.app` |

They serve the read-only methods in [calling-contracts.md](calling-contracts.md)
(`zalkanes_getInfo`, `zalkanes_view`, `zalkanes_getContract`,
`zalkanes_getCode`, `zalkanes_getExecution`, `zalkanes_getBlockExecutions`).
There is no authentication and no rate limiting; be reasonable. Check the
endpoint you are about to use:

```bash
curl -s -X POST https://zalkanes-testnet-rc2-a-production.up.railway.app \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"zalkanes_getInfo","params":[]}'
```

`protocol_manifest_hash` must equal the value above and `indexer.state` must
be `healthy` or `syncing`. Below the activation height the state root is the
empty root `b120099c…`; that is expected. Both endpoints can be compared with
`scripts/testnet-two-node-check.sh <A> <B>`.

Deploying and calling need a Zcash node to broadcast through, and Zalkanes
does not proxy transactions: you run your own Zebra testnet node (below).
Any Zalkanes node you run yourself is equivalent to the endpoint above; at
equal heights every node reports the same state root.

## Deploy your first contract to public testnet

Every command exists in the CLI as shipped; the deploy, call, and view steps
only succeed once the network status above is `ACTIVE`. Until then you can
do everything up to and including step 6 on testnet, and the whole flow on
regtest ([quickstart.md](quickstart.md)).

### 1. Clone and build

```bash
git clone https://github.com/whatnotbot/zalkanes.git
cd zalkanes
cargo build --release
export PATH="$PWD/target/release:$PATH"
```

Rust 1.88 and the `wasm32-unknown-unknown` target are pinned by
`rust-toolchain.toml`; `rustup` installs them on first use. RocksDB needs a C
toolchain (`clang`).

### 2. Copy the counter as a template

```bash
cp -r contracts/counter contracts/my-contract
```

Edit `contracts/my-contract/Cargo.toml` and change `name = "counter"` to
`name = "my-contract"`. Leave the rest: `crate-type = ["cdylib"]`, the empty
`[workspace]` table, the `zalkanes-sdk` path dependency, and the release
profile (`opt-level = "s"`, `panic = "abort"`, `strip = true`).

### 3. Write `dispatch(opcode, input_len)`

Replace `contracts/my-contract/src/lib.rs`. The entrypoint is exactly this
signature; return `0` for success and anything else to fail the call (and
discard its writes):

```rust
#![no_std]
#![no_main]

#[global_allocator]
static ALLOC: wee_alloc::WeeAlloc = wee_alloc::WeeAlloc::INIT;
extern crate alloc;

use zalkanes_sdk as sdk;

const KEY: &[u8] = b"value";

#[no_mangle]
pub extern "C" fn dispatch(opcode: i32, _input_len: i32) -> i32 {
    match opcode as u16 {
        // 1: set(value: u64 big-endian) -- input must be 8 bytes
        1 => {
            let input = sdk::read_input();
            if input.len() != 8 {
                return -1;
            }
            if sdk::set(KEY, &input) {
                0
            } else {
                -1
            }
        }
        // 2: get() -> u64 big-endian (0 if never set)
        2 => {
            let v = sdk::get(KEY).unwrap_or_else(|| sdk::u64_to_bytes(0).to_vec());
            if sdk::write_output(&v) {
                0
            } else {
                -1
            }
        }
        _ => -1,
    }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}
```

Rules that will otherwise bite you: no `std`, no floating point, no
threads, no imports beyond the six SDK host functions; storage keys up to
256 bytes and values up to 64 KiB; inputs sent from the CLI up to 38 bytes.
Full list: [wasm-rules.md](wasm-rules.md), [sdk.md](sdk.md).

### 4. Build the WASM

```bash
zalkanes contract build --manifest-path ./contracts/my-contract
```

It prints the artifact path,
`./contracts/my-contract/target/wasm32-unknown-unknown/release/my_contract.wasm`.
The deploy step validates the module with the same rules every node applies,
so an invalid module fails before any funds move.

### 5. Run your own Zebra testnet node

The CLI broadcasts through a Zcash JSON-RPC endpoint you control. Run
`zebrad` v6.3.0 on the public testnet with RPC enabled and let it sync
(hours; the chain is about 50 GB). See `docs/operator-guide.md` for the
config. The examples below assume its RPC is at `http://127.0.0.1:18232`.

### 6. Obtain test ZEC and configure testnet

Generate a fresh 32-byte hex signing key (for example
`openssl rand -hex 32`), never reuse the regtest dev key, never commit it.
Then:

```bash
export ZALKANES_NETWORK=testnet
export ZALKANES_ZCASH_RPC_URL=http://127.0.0.1:18232
export ZALKANES_URL=https://zalkanes-testnet-rc2-a-production.up.railway.app
export ZALKANES_SIGNING_KEY=<your 64-hex key>
zalkanes contract fund
```

On testnet `contract fund` does not mine; it prints the transparent address
derived from your key and stops:

```text
Error: contract fund only mines blocks on regtest; on test send funds to the transparent funding address tmLomwDq… from a faucet, then set ZALKANES_FUNDING_TXID/VOUT
```

Send test ZEC (TAZ) to that address from a Zcash testnet faucet. A counter
deploy plus a few calls costs well under 0.01 TAZ in fees. Once the faucet
transaction is mined, tell the CLI which output to spend:

```bash
export ZALKANES_FUNDING_TXID=<faucet transaction id>
export ZALKANES_FUNDING_VOUT=0
```

(`ZALKANES_FUNDING_VOUT` is the output index that pays your address; `0` is
usual.) Shielded funding through the wallet is also supported, see
[wallet-funding.md](wallet-funding.md).

### 7. Deploy

```bash
zalkanes contract deploy ./contracts/my-contract/target/wasm32-unknown-unknown/release/my_contract.wasm --funding transparent --yes --wait
```

Two testnet transactions are built, signed, and broadcast through your
Zebra: PREPARE, then DEPLOY. The CLI waits for each to get one confirmation
(testnet blocks are about 75 seconds apart), then polls the Zalkanes endpoint
until the contract is indexed. `--dry-run` shows the plan and fees without
broadcasting.

### 8. Copy the ContractId

```text
DEPLOY mined at height 4346…: <txid>
ContractId: <64 hex characters>
indexed: {"code_hash":"…","code_size":…,"contract_id":"…"}
```

```bash
CONTRACT_ID=<the ContractId line>
```

The id is derived from the DEPLOY transaction and the code hash; the
`indexed:` line proves the public node derived the same id.

### 9. Call

```bash
zalkanes contract call "$CONTRACT_ID" 1 000000000000002a --funding transparent --yes --wait
```

Sets the value to 42 (`0x2a`) with one testnet transaction. `--wait` prints
the execution record; `"success":true` means your writes were committed.
A failed execution (`"success":false`) still spends the fee and changes
nothing.

### 10. View

```bash
zalkanes contract view "$CONTRACT_ID" 2 "" --rpc-url https://zalkanes-testnet-rc2-a-production.up.railway.app
```

```json
{"jsonrpc":"2.0","id":1,"result":{"success":true,"output_hex":"000000000000002a",…}}
```

Views are free, broadcast nothing, and run against the node's current
indexed state. Ask node B the same question to confirm independence.

## Running your own Zalkanes node

Anyone can reproduce the public state. With your synced Zebra:

```bash
ZALKANES_NETWORK=testnet ZALKANES_RPC_URL=http://127.0.0.1:18232 \
ZALKANES_DATA_DIR=./zalkanes-data zalkanes node serve --port 3030
```

The node fast-forwards to the activation height and indexes from there.
At equal heights its `zalkanes_getInfo` state root must equal the public
endpoint's; `scripts/testnet-two-node-check.sh http://127.0.0.1:3030 https://zalkanes-testnet-rc2-a-production.up.railway.app`
checks that, plus the manifest hash and indexer liveness.

## Debugging

`zalkanes_getExecution` with a txid (display order) shows whether a call
succeeded, its fuel, output, and error. `zalkanes_getBlockExecutions` lists
every execution in a block. `zalkanes_getInfo` shows whether the node is
syncing and whether its indexer is healthy. A Zcash testnet block explorer
shows the raw transaction, its OP_RETURN, and (for DEPLOY) the carrier
inputs. Common errors: [troubleshooting.md](troubleshooting.md).

## Deployment runbook and evidence

- `docs/release/testnet-rc2-deployment.md`: the operator procedure for the
  two clean RC2 nodes, the agreement gate, and the one-time acceptance.
- `audit/TESTNET-ACTIVATION-RC2-EVIDENCE.md`: evidence as steps are executed.
- `audit/RC2-DECISION-RECORD.md`: how 4,346,500 was chosen.
- `docs/release/testnet-activation.md`, `audit/TESTNET-EVIDENCE.md`: the
  earlier pre-freeze deployment (activation 4,338,100, RC1 manifest). Its
  endpoint and state are superseded and are not RC2.

Mainnet is disabled by construction: `mainnet_activation_height = "None"` in
the manifest and `MAINNET_ACTIVATION_HEIGHT = None` in the code, and every
money-moving mainnet command also requires `--confirm-mainnet`.
