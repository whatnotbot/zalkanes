# Live acceptance evidence — shielded wallet + two-node determinism

> **Scope caveat (see `TESTNET-ACTIVATION-AUDIT.md`):** this is acceptance of
> the FROZEN CODE on public testnet. It is **not** a fresh frozen-RC
> activation — the testnet activation height was frozen ~16 hours before the
> protocol was, so some pre-freeze history above it is read under frozen
> rules. The resulting state is nonetheless well-defined and was proven by an
> independent empty-database resync.

Date: 2026-09-11/12 (UTC+7 timestamps). Network: **Zcash public testnet**
(NU6.3 / Ironwood active). Protocol: v0 `FROZEN-RC`, manifest hash
`57178628cebadad21da5e5c6495a7d646a55c0299609ea8939737744ab0b8752`
(verified live via `zalkanes_getInfo` before and during acceptance).
Contains NO seeds, spending keys, or wallet secrets.

## Wallet funding (previously established)

- Funding tx `7d0fd2bc83c019cacac3f700ce113368d49cd0259f749a25233a7ad369ab12c5`,
  note mined at height 4,339,325, pool **Ironwood**, 10,000,000 zat,
  wallet birthday 4,339,225.

## Original counter contract `607a6246c512239a23f51cf8053444d4d76e7684c1a53dc26a626b474a8cf3c0`

Initial state confirmed live before broadcasting: `get() == 2`
(fuel 3936, state root `9c2842d3453e1b1532eba1303801c23f6b0c41ffbf43ce958b9e42af49cd3410`).

### Shielded CALL #1 (2 → 3)

| field | value |
|---|---|
| txid | `fe266af59918685ccb7fc24362375077dc30c75186289e8ecf296aecd59d7fd5` |
| mined | height 4,339,397, block `0099ce57d63601a929f51efa9377104c6de4bd33905ca47325a71acfa0488437` |
| size / version | 9,219 bytes, v6 |
| funded pool | Ironwood (single note, 10,000,000 zat) |
| fee | 20,000 zat |
| change | 9,980,000 zat → Ironwood (internal) |
| execution | success, fuel 3,936, `get() == 3` |
| state root | `9c2842d3…` → `ebd00d5e1061491be689fcd6ae34f8af6cca8fc17e5df46043ea7ff6cb5a2b8e` |

### Shielded CALL #2 (3 → 4) — spends CALL #1's change note

| field | value |
|---|---|
| txid | `58ee8880c4ccc13e91f2483eb3210f4df8ec0619a515b44b3d8e55ee1a16a9a0` |
| mined | height 4,339,401, block `008fae5c95f56c8c17c9b8cff9085e0b7ac476453911a9da16b0f72068318460` |
| fee | 20,000 zat |
| change | 9,960,000 zat → Ironwood |
| execution | success, fuel 3,936, `get() == 4` |
| state root | `ebd00d5e…` → `0c1ef3e47664b433cb50b939df864f535fbac7ade7f8d63f7691c89c89cbb574` |
| continuity | CALL #2 `state_root_before` == CALL #1 `state_root_after` |

## ZALK payload equivalence (Item 13)

The live-broadcast CALL payload
`5a414c4b0002607a6246c512239a23f51cf8053444d4d76e7684c1a53dc26a626b474a8cf3c000010000`
is asserted byte-identical between transparent-funded and shielded-funded
plans (payload AND OP_RETURN scriptPubKey) by
`byte_equality_transparent_vs_shielded_call`, which pins these exact bytes.
Outer transaction bytes are NOT required to match (different funding).

## Fresh shielded-funded deployment (Item 14)

| stage | value |
|---|---|
| PREPARE (shielded) | txid `93c88c718b7c96e898e8f230663d451e4614d36adfec394591f5e7cd90093f9f`, mined 4,339,405, fee 20,000 zat, 2 carriers × 72,500 zat, change 9,795,000 → Ironwood |
| DEPLOY (transparent carriers) | txid `dd9b8cce0e06e38b884675937a8116ccc86446ab209c542e0f99a1bf78c6c14e`, mined 4,339,406, fee 95,000 zat; carrier scriptSig content + deployment byte-stream reconstruction verified pre-broadcast |
| ContractId | `0af32d54e01177d3eba37664ec47f6fd46728d813115ac0124b997aebdbf34c9` (locally derived == indexer record) |
| WASM | code hash `fa8289fbc0fdb132e57f939033a9971b0ee2990ec49db247aad3f682aa804cc7`, 2,513 bytes (canonical counter) |
| fresh state | `get() == 0` at root `de9236b7e7bb3eabfcca559dd38158afa42e94d441554aedf437396669f17e7c` |
| CALL 0→1 | txid `998b1f02c3d5686cd41b23689b0fc3bfb65882490d9200bdf112916dc954c6bd`, mined 4,339,407, root `05c8e72c88b38683e9ec008cf89adb551e786f606288e9a428c52d5c431602d8` |
| CALL 1→2 | txid `bf07d19f126697d0d55711b0891d226d5e7d176c76c40408457a1127cf50b38f`, mined 4,339,409, root `28256d5f65020dddcdd5befc70f4d80f25de605232765980c3f87c1136e86f95` |

Fund safety: wallet retained ≈ 9,755,000 zat shielded + 50,000 zat
transparent change after the full acceptance run.

## Two-independent-node root equality (Item 16.11, live form)

A fresh local Zalkanes indexer (node B, empty RocksDB, this machine)
resynced the ENTIRE post-activation chain (1,435 blocks from activation
4,338,100) through the project Zebra RPC, fully independently of the
Railway indexer (node A, its own DB):

- node A (`zalkanes-testnet-production.up.railway.app`):
  height 4,339,534, root `28256d5f65020dddcdd5befc70f4d80f25de605232765980c3f87c1136e86f95`
- node B (local resync): height 4,339,534, root
  `28256d5f65020dddcdd5befc70f4d80f25de605232765980c3f87c1136e86f95`
- **byte-identical**, including reproducing every execution of the live
  acceptance above during resync (roots `9c2842d3… → ebd00d5e… →
  0c1ef3e4… → de9236b7… → 05c8e72c… → 28256d5f…` all re-derived).

Scale stress (in-process form, real block processor, two isolated state
databases): 100 deployments, 10,000 successful calls, 1,000 intentional
failures across ~651 blocks — roots byte-identical at every block
(`two_node_stress.rs`, release profile).

## Cross-architecture / cross-profile determinism (Item 16.10)

The consensus test set — including verification of the 101 committed
execution vectors (`test-vectors/execution/v1.json`) — is green in four
configurations on this host — aarch64 debug + release (native Apple
Silicon) and x86_64 debug + release (**Rosetta 2 emulation, labeled as
such**) — and in two NATIVE Linux CI configurations: x86_64
(`CI` workflow) and **aarch64** (`arm64-determinism` workflow, GitHub-hosted
`ubuntu-24.04-arm`), each in debug and release. Native ARM64 Linux is
therefore covered and is no longer an external blocker.

## Fuzz campaign (Item 16.8)

See `fuzz/README.md` for the full log: 412M+ executions across five
targets; two findings (state rollback round-trip; wasmi translator panic
containment), both fixed with regression seeds and unit regressions;
re-runs clean; zero unresolved findings.

## Preserved gaps (honest)

- ~~**Live note-level reorg evidence**~~ — **CLOSED**. The competing-chain
  machinery now exists: two real zebrad v6.3.0 nodes, a real shielded coinbase
  note, a real spend, and a real best-work reorg. See
  `audit/LIVE-REORG-EVIDENCE.md` (CI run 34682825442). One OPEN observation
  (a stale note row) is recorded there. Still regtest only — the public
  testnet has never been deliberately reorganised.
- **Fresh frozen-RC testnet activation** — see `TESTNET-ACTIVATION-AUDIT.md`.
  Not performed; requires explicit approval.
- **Non-emulated x86_64-macOS** runs (native x86_64 Linux is covered by CI).
- **External security audit** — hard mainnet gate.
