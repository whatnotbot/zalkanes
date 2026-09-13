# Zalkanes public testnet — RC2 status for developers

**Status: ACTIVATION PENDING.** Protocol v0 (RC2) activates on the
public Zcash testnet at block height **4,346,500** (≈ 2026-09-17 UTC).
Until that height the network state is empty by design and deployments
are not yet interpreted. After that height, anyone can deploy and call
contracts. **Mainnet is NOT activated and must not be used.**

## The RC2 identity (pin these)

```
git tag:        audit-candidate-v0-rc2  (signed)
commit:         ed15911d3ac39a645fd9dc064f85ce879cb729f2
manifest hash:  06e3df62e5e98a3c276b583e038cbea8d05934e9d2a8e7c00299fec3140bf4bb
activation:     4,346,500 (Zcash testnet)
zebra:          v6.3.0
```

Any node reporting a different `protocol_manifest_hash` in
`zalkanes_getInfo` is out of consensus — do not trust it.

## Public read endpoints (two independent RC2 nodes, clean databases)

```
https://zalkanes-testnet-a2-production.up.railway.app
https://zalkanes-testnet-b-production.up.railway.app
```

JSON-RPC methods: `zalkanes_getInfo`, `zalkanes_getStateRoot`,
`zalkanes_getContract`, `zalkanes_getCode`, `zalkanes_view`,
`zalkanes_getExecution`, `zalkanes_getBlockExecutions`. Example:

```bash
curl -s -X POST -H "Content-Type: application/json" \
  --data '{"jsonrpc":"2.0","id":1,"method":"zalkanes_getInfo","params":[]}' \
  https://zalkanes-testnet-a2-production.up.railway.app
```

Cross-check both nodes; their `state_root` at equal height must be
byte-identical. Public endpoints are a convenience — the trust anchor is
always your own node (below).

## Run your own node (recommended)

```bash
git clone https://github.com/whatnotbot/zalkanes && cd zalkanes
git checkout audit-candidate-v0-rc2
cargo build --release -p zalkanes-cli

# Point at your own Zebra v6.3.0 testnet validator's RPC:
export ZALKANES_NETWORK=testnet
export ZALKANES_RPC_URL=http://127.0.0.1:18232
export ZALKANES_DATA_DIR=$HOME/.zalkanes-testnet   # fresh dir, RC2 only
./target/release/zalkanes node serve --port 3030
```

The node fast-forwards to 4,346,500 with the empty state root
`b120099c167da673588b15aa827c3bbd9339a9a2d934b0d20c99cebe69f8ffe6`,
then indexes normally. Never reuse a database from any pre-RC2 build.

Operational note: if `indexed_height` stops following
`chain_tip_height`, restart the process (a known non-consensus
serve-loop defect in RC2; state is unaffected and resumes exactly).

## Deploy and call a contract (after activation)

```bash
# 1) wallet (testnet only; guard the seed)
zalkanes wallet create
zalkanes wallet address

# 2) fund with TAZ from a public faucet (e.g. the Jino Labs faucet),
#    then verify through your own node:
zalkanes wallet scan && zalkanes wallet balance

# 3) build + deploy a contract (two transactions: PREPARE then DEPLOY)
zalkanes contract build --manifest-path contracts/counter
zalkanes contract deploy target/wasm32-unknown-unknown/release/counter.wasm \
  --funding shielded --dry-run          # inspect first
zalkanes contract deploy target/wasm32-unknown-unknown/release/counter.wasm \
  --funding shielded --yes --wait       # broadcast

# 4) call and read it
zalkanes contract call <CONTRACT_ID> 0x0001 --funding shielded --yes --wait
zalkanes contract view <CONTRACT_ID> 0x0002
```

Fees follow ZIP-317; the deploy prints the derived ContractId, which
must match what the indexer records (`zalkanes_getContract`).

## Limits (v0)

WASM ≤ 256 KiB; imports = the six-function host ABI (`storage_get/set/
delete`, `context_block_height`, `input_read`, `output_write`); no
floats; fuel 10M/call. Each call in a block reads pre-block state (one
effective state transition per contract per block — design your v0
contracts accordingly).

## Mainnet

`MAINNET_ACTIVATION_HEIGHT = None`. The CLI refuses mainnet without an
explicit confirm flag, and no mainnet deployment of any kind is
authorized.
