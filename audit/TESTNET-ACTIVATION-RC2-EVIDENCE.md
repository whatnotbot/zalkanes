# RC2 public-testnet activation evidence

Evidence for the fresh `audit-candidate-v0-rc2` public-testnet activation
(runbook: `docs/release/testnet-rc2-deployment.md`). Sections are appended
as steps are executed; nothing is recorded before it happens. No seeds, keys,
or passphrases appear here.

## 1. Infrastructure provisioning (executed 2026-09-14, owner-authorised)

**Scope executed:** provisioning only. No DEPLOY/CALL transaction was
broadcast, no testnet ZEC was spent, `protocol/v0.toml` and
`MAINNET_ACTIVATION_HEIGHT` are unchanged, mainnet untouched.

| item | value |
|---|---|
| source commit (both nodes, Railway deployment meta) | `2243878f5f97e75a23c49b26c3280d63c7e66a22` (`main`) |
| protocol manifest advertised by both nodes | `06e3df62e5e98a3c276b583e038cbea8d05934e9d2a8e7c00299fec3140bf4bb` |
| testnet activation height | 4,346,500 (not yet reached at provisioning) |
| mainnet activation | None |
| validator | existing `zebra-testnet` (zebrad v6.3.0, volume `24414a2f…` untouched) |
| node A | Railway service `zalkanes-testnet-rc2-a` (`081657c0-8fab-43c9-8a66-56373fdced03`), new empty volume `zalkanes-testnet-rc2-a-volume` (`a9dd99ed-2ab3-4719-a543-4615e09a9c3c`) at `/data/zalkanes`, domain `zalkanes-testnet-rc2-a-production.up.railway.app` → port 3030 |
| node B | Railway service `zalkanes-testnet-rc2-b` (`0751d72c-c476-4701-bbab-912f95f13818`), new empty volume `zalkanes-testnet-rc2-b-volume` (`a745546f-2be7-49c9-85b4-9839a566e6d2`) at `/data/zalkanes`, domain `zalkanes-testnet-rc2-b-production.up.railway.app` → port 3030 |
| variables (both) | `ZALKANES_NETWORK=testnet`, `ZALKANES_DATA_DIR=/data/zalkanes`, `ZALKANES_RPC_URL=http://zebra-testnet.railway.internal:18232`, `RUST_LOG=zalkanes=info`, `PORT=3030` |
| build | root `Dockerfile` (`cargo build --release --locked -p zalkanes-cli`), Railway deployments `SUCCESS` at 14:28 UTC |

Startup logs (both nodes, identical apart from timestamps): chain source
connection verified (`chain="test"`, height 4,345,997), JSON-RPC on 3030,
health on 3031, then one `fast-forwarded below activation height` commit at
4,345,997. Zero `transient upstream failure` lines, zero
`indexing loop failed` lines.

### `zalkanes_getInfo` at provisioning

Node A:

```json
{"jsonrpc":"2.0","id":1,"result":{"protocol_version":0,"protocol_manifest_hash":"06e3df62e5e98a3c276b583e038cbea8d05934e9d2a8e7c00299fec3140bf4bb","network":"test","indexed_height":4345998,"chain_tip_height":4345998,"indexed_block_hash":"0000c75571868223ff5636f5ec4a0fe603cd2fa2162e9417733f94dcfb0e373b","state_root":"b120099c167da673588b15aa827c3bbd9339a9a2d934b0d20c99cebe69f8ffe6","syncing":false,"indexer":{"state":"healthy","alive":true,"advancing":true,"indexed_height":4345998,"tip_height":4345998,"last_progress_secs_ago":2,"last_tick_ok_secs_ago":2,"consecutive_failures":0,"total_failures":0,"last_error":null,"dead_reason":null}}}
```

Node B:

```json
{"jsonrpc":"2.0","id":1,"result":{"protocol_version":0,"protocol_manifest_hash":"06e3df62e5e98a3c276b583e038cbea8d05934e9d2a8e7c00299fec3140bf4bb","network":"test","indexed_height":4345998,"chain_tip_height":4345998,"indexed_block_hash":"0000c75571868223ff5636f5ec4a0fe603cd2fa2162e9417733f94dcfb0e373b","state_root":"b120099c167da673588b15aa827c3bbd9339a9a2d934b0d20c99cebe69f8ffe6","syncing":false,"indexer":{"state":"healthy","alive":true,"advancing":true,"indexed_height":4345998,"tip_height":4345998,"last_progress_secs_ago":2,"last_tick_ok_secs_ago":2,"consecutive_failures":0,"total_failures":0,"last_error":null,"dead_reason":null}}}
```

### Two-node agreement (`scripts/testnet-two-node-check.sh`)

```text
expected manifest: 06e3df62e5e98a3c276b583e038cbea8d05934e9d2a8e7c00299fec3140bf4bb (testnet activation 4346500)
node 0: network=test height=4345998 tip=4345998 indexer=healthy root=b120099c167da673… block=0000c75571868223…
node 1 (https://zalkanes-testnet-rc2-b-production.up.railway.app): unreachable
  (attempt 1/6)
node 0: network=test height=4345998 tip=4345998 indexer=healthy root=b120099c167da673… block=0000c75571868223…
node 1: network=test height=4345998 tip=4345998 indexer=healthy root=b120099c167da673… block=0000c75571868223…
AGREEMENT: 2 nodes at height 4345998, block 0000c75571868223ff5636f5ec4a0fe603cd2fa2162e9417733f94dcfb0e373b, root b120099c167da673588b15aa827c3bbd9339a9a2d934b0d20c99cebe69f8ffe6
```

Both nodes: same indexed height, same indexed block hash, same state root,
which is the empty-state root `b120099c…` as required below activation.

### Observations recorded honestly

- Railway auto-deploys GitHub-sourced services on every push to `main`.
  Merging PRs #7 and #9 therefore redeployed `zebra-testnet`,
  `zalkanes-testnet`, and `zebra` at `2243878` before provisioning began.
  `zebra-testnet` restarted on its persisted volume and finished its sync to
  the tip (4,345,989 at 14:27 UTC). No volume was wiped or replaced.
- The superseded indexers (`zalkanes-testnet`, `zalkanes-testnet-b`,
  `zalkanes-testnet-a2`) were **not** retired: the runbook retires them after
  the on-chain acceptance (§8), which is not yet authorised.
- Volumes report 0 MB used immediately after the first commit; RocksDB's
  footprint at one empty block is below the reporting granularity.

## 2. Activation and on-chain acceptance

Not executed. Requires separate owner authorisation.
