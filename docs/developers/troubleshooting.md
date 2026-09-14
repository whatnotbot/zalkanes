# Troubleshooting

Messages are quoted as the CLI prints them.

## Building

**`error: the manifest-path must be a path to a Cargo.toml file`** (from
`cargo` directly). Point `--manifest-path` at the `Cargo.toml`, or use
`zalkanes contract build --manifest-path <crate-dir>`, which accepts either.

**`error[E0463]: can't find crate for 'core'` / target not installed.** Run
`rustup target add wasm32-unknown-unknown` (the pinned toolchain file adds
it automatically when `rustup` manages your toolchain).

**RocksDB / `libclang` errors when building the node.** Install a C toolchain
(`clang`, `libclang`); on macOS `xcode-select --install`.

**A contract `Cargo.lock` changed after building.** The example lockfile was
stale; discard the change (`git checkout -- contracts/<name>/Cargo.lock`) or
commit it deliberately.

## Starting the environment

**`ERROR: neither a zebrad binary nor docker compose is available.`** Put
`zebrad` v6.3.0 on your `PATH` (or set `ZEBRAD`), or install Docker. See
[local-regtest.md](local-regtest.md#getting-zebrad).

**`ERROR: ./target/release/zalkanes not found; run: cargo build --release`.**
Build first, or set `ZALKANES_BIN`.

**`Zebra regtest RPC … did not come up`.** Read `.regtest/zebrad.log`. A
stale `zebrad` from an earlier session holding port 18232 is the usual
cause; `scripts/run-regtest.sh stop` then `start` again, or set
`ZALKANES_REGTEST_FRESH=1`.

**`ZALKANES_RPC_URL not set`.** `node serve` and `node status` need the Zebra
endpoint. The launcher sets it; when running the node by hand, export it.

**`ZALKANES_ZCASH_RPC_URL not set`.** `contract deploy/call/fund` and the
wallet need it. `eval "$(./scripts/run-regtest.sh env)"`.

## Deploying and calling

**`WASM validation failed: unresolvable import env::<name> (host ABI provides only env::{…})`.**
The module imports something outside the six v0 host functions. The
`caller` example does this on purpose. See [wasm-rules.md](wasm-rules.md).

**`WASM validation failed: module too large` / `function count … exceeds`.**
Over a structural limit; see [protocol-limits.md](protocol-limits.md).

**`WASM validation failed: WASM parse error: …`.** Usually floating point
(`f32`/`f64` instructions) or a forbidden proposal. Check that no dependency
pulls in float formatting; build with `opt-level = "s"` and `panic = "abort"`.

**`input is N bytes but an inline CALL carries at most 38 bytes`.** The CLI
sends inline CALL messages only. Shrink the input or split the operation
across calls ([calling-contracts.md](calling-contracts.md#input-size)).

**`broadcast rejected: rejected (code -25): transaction is non-standard`.**
Zebra refused the transaction under standardness policy. With the shipped
CLI this should not occur for a call within the input limit or a validated
deploy; if it does, keep the output and open an issue.

**`contract id must be 32 hex bytes`.** Pass the 64-hex `ContractId` printed
by deploy, nothing else.

**`contract <id> was not indexed within 10 minutes`** (with `--wait`). The
Zalkanes node is not running, is pointed at a different Zebra, or is behind.
Check `zalkanes_getInfo`: `indexed_height` should reach the height printed
in `DEPLOY mined at height N`. `ZALKANES_URL` must point at the node.

**`Proceed? [y/N]` and nothing happens in a script.** Add `--yes`; a
non-interactive stdin reads as "no".

**`refusing a mainnet money-moving command without --confirm-mainnet`.**
Expected: mainnet execution is disabled and this guard exists to stop
accidental spends. Use regtest or, once active, testnet.

**A call "did nothing".** Look at the execution record: `--wait` prints it,
or query `zalkanes_getExecution` with the txid. `"success":false` with
`"error":"dispatch returned error code -1"` means the contract rejected the
call (unknown opcode, short input, missing key, …); the transaction was
still mined and the fee spent. See [contract-model.md](contract-model.md#failure-traps-and-fuel-exhaustion).

**`fuel exhausted`.** The call ran past its fuel budget. Bound your loops;
budgets are in [protocol-limits.md](protocol-limits.md).

## Viewing and the node

**`{"result":{"success":false,…,"error":"invalid contract id"}}`.** The id is
not 64 hex characters.

**`"error":"contract not found"`.** Well-formed id, but this node has not
indexed such a deploy. Check `indexed_height` against the deploy height, and
that both use the same network.

**`failed to open RocksDB at … (lock held too long)`** from
`zalkanes state-root` or `zalkanes node status`. Those commands open the
database directly and cannot while `node serve` holds it. Use
`zalkanes_getStateRoot` / `zalkanes_getInfo` over RPC, or stop the node.

**`indexed_height` stays behind `chain_tip_height`.** The node is syncing;
on regtest it catches up within a few seconds. If it never does, look at
`indexer` in `zalkanes_getInfo`: `state: "stalled"` with a `last_error`
means upstream calls keep failing (the loop retries every poll and never
exits on those); `state: "dead"` means a fatal local state error, and the
process exits non-zero. `GET /ready` on the health port returns 503 in both
cases even though `/health` is still 200. Read `.regtest/zalkanes.log`.

**Two nodes disagree on `state_root` at the same height.** They run different
code, a different `protocol_manifest_hash`, or a divergent Zebra. Compare the
manifest hash first; then reindex the suspect node from an empty data
directory.

## Wallet

**`no wallet at … — run `zalkanes wallet create` first`.** Wallet commands
need a wallet in `ZALKANES_WALLET_DIR`.

**`wallet is LOCKED: run `zalkanes wallet unlock` before a shielded spend`.**
Unlock (default 900 s), then retry; the passphrase is still required per spend.

**`--funding shielded requires a wallet`.** No wallet exists; create one or
use `--funding transparent`.

**`wallet scan` is slow on regtest.** It scans every block since the wallet's
birthday, and each CLI operation on regtest mines about 111 blocks; expect
minutes for a long regtest chain. The regtest developer flow uses transparent
funding and does not need a wallet.

## Still stuck

`RUST_LOG=zalkanes=debug` on the node, `--dry-run` on the CLI, and the
execution record are the three tools that answer most questions. Consensus
questions belong in `docs/protocol-v0.md` and `docs/wasm-consensus.md`;
operational ones in `docs/operator-guide.md`.
