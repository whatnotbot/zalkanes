# Live competing-branch reorg evidence

Closes two mainnet hard gates that the deterministic reorg matrix could not:
**live note-level competing-branch reorg** and **activation-boundary live
reorg**. Both run in CI on every push, against real software.

Workflow: `.github/workflows/reorg.yml` (`competing-branch-regtest`).
Passing run: **34682825442** — all three jobs `success`.
Contains no seeds, spending keys, or passphrases. The regtest seed is a
published throwaway for an ephemeral private chain and holds no value.

## What makes this real

| element | what is actually used |
|---|---|
| Nodes | two independent **zebrad v6.3.0** processes (pinned release, checksum-verified) |
| Blocks | produced by Zebra's own miner via `generate` / `generatetoaddress` |
| Reorg | Zebra's own best-work chain selection, triggered by `submitblock` |
| Shielded note | a real **Ironwood** note from a real shielded coinbase |
| Spend | a real proved-and-signed shielded transaction through the production plan → prove → sign → `VerifiedTransaction` path |
| Wallet | the real `SqliteShieldedWallet`, driving the real `rewind_to_chain_state` + rescan |
| Indexer | the real `zalkanes node serve` loop over a real `RocksState` |

No wallet SQLite row and no RocksDB key was ever written by hand. No public
testnet was touched.

### Why a second node, and not `invalidateblock`

Zebra exposes `invalidateblock`, which is the cheap way to fake a reorg. It is
deliberately **not** used: it marks the abandoned branch permanently invalid,
which is not what a real competing chain does. Instead a second node mines an
independent branch from the shared genesis and its blocks are submitted to the
first node, which reorganises purely because that branch carries more work.

Two Zebra facts make this sound: regtest disables proof-of-work but still
enforces `nBits` exactly, so **every regtest block carries identical work and a
longer chain is strictly heavier**; and `getblock` is best-chain-only, so each
branch's raw blocks are harvested while that branch is still its own node's
best chain.

## Topology

```
  common ancestor H
        |
        +---- branch A ----  (mined on node A; carries our transaction)
        |
        +---- branch B ---- … longer …   (mined on node B; does not)
```

## CASE A — a received note disappears with its branch

| field | value |
|---|---|
| common ancestor | height **12** |
| branch-A note block | height **13**, `07b8ab3940e252b4be2bafd1d3eb3aac8238000a352cf60482995e60584efbb2` |
| note before reorg | visible, **625,000,000 zat**, pool **Ironwood**, `ironwood_notes=1` |
| wallet scan identity before | height 14, matching node A's canonical tip |
| branch B | 4 blocks from height 12, overtaking branch A's 2 |
| canonical tip after reorg | **16** `c4e315db2b234425e15d87cfaa69ceb3a60da893ac4ae269ef0091b69dd55c83` |
| note after reorg | **not spendable — balance 0 zat** |
| note's block after reorg | no longer canonical at height 13 |
| spend planner after reorg | **refuses**: `no spendable shielded notes covering 10000 zat` |
| restart | re-verified after reopening from disk: still 0 zat |

### Open observation — a stale note row survives the reorg

After the reorg the spendable balance is correctly **0**, and the production
spend planner correctly **refuses** to build any transaction. But
`shielded_note_summary()` still reports `ironwood_notes=1` for a note whose
block is no longer canonical.

- **Not a fund-safety defect on the evidence available.** Both money paths are
  correct: `balance()` (via `get_wallet_summary`) reports 0, and
  `select_spends` (via `select_spendable_notes`) refuses to select it. The test
  asserts the refusal explicitly, so a regression that made the note selectable
  would fail CI.
- **It is a reporting discrepancy**, in a diagnostic counter that uses a
  different upstream query (`select_unspent_notes` with `NoteRequest::Unspent`)
  than either money path.
- **Root cause not yet established.** It is not yet determined whether the note
  row is retained deliberately by upstream truncation semantics (a transaction
  that could in principle be re-mined) or is genuinely stale. For a *coinbase*
  note it can never be re-mined, so retention would be pointless here.
- **Status: OPEN**, recorded for the external auditor. It is deliberately not
  written off as cosmetic without a root cause.

## CASE B — a spend rolls back and the note returns

| field | value |
|---|---|
| note block | height **13**, on history **common** to both branches |
| common ancestor | height **15** |
| spend txid | `108e5a3e03f332407099da2db1aa2eb8508a06a67381f7568df705d5b11d7f9f` |
| spend mined | height 16 on branch A, 1 confirmation |
| balance while spent | **624,980,000 zat** (the change note) |
| branch B | 45 blocks from height 15 → tip **60** |
| canonical tip after reorg | **60** `21f58b4e142f843900ac994d3663f9f2995519a9fcb36ab237f746e728a0c19b` |
| spend after reorg | **0 confirmations** — no longer mined |
| branch-A change note | **gone** (`ironwood_notes=0`) |
| after releasing the dead plan | **625,000,000 zat restored**, `ironwood_notes=1` |
| restart | balance preserved |

### Why branch B is 45 blocks, and why a release step is needed

Node A keeps the reorged-out spend in its **mempool**, where it stays valid —
its anchor is on the common history — so it could legitimately be mined again.
Two consequences, both of which the test handles honestly rather than
engineering around:

1. Branch B is extended past the transaction's **expiry height** (56 here), so
   the spend is permanently dead. Those blocks are mined on node B, which never
   saw the spend and therefore cannot re-include it.
2. Immediately after the reorg the note is unspent in canonical history but
   still held by the wallet's own **note reservation**. That is correct
   behaviour while a broadcast might still land, and it is exactly what
   production reconciliation resolves. The test drives the real release path
   (`ShieldedWallet::release`) and only then asserts the note is spendable.

So "spendable again" here means: *after the canonical chain has made the spend
impossible and the wallet has reconciled its own in-flight record* — not that
the wallet naively re-offers a note it might still be spending.

## Activation-boundary reorg

Run in the same workflow, driven by `scripts/activation-boundary-reorg.sh`
against the real `zalkanes node serve` loop.

The Zalkanes regtest activation height is **120** for this run, set through
`ZALKANES_REGTEST_ACTIVATION_HEIGHT`. See "About the override" below.

| stage | height | state root |
|---|---|---|
| pre-activation tip | 119 | `b120099c167da673588b15aa827c3bbd9339a9a2d934b0d20c99cebe69f8ffe6` (empty) |
| crossed activation, no protocol txs | 122 | `b120099c…` — **unchanged**, as required |
| after DEPLOY | 236 | `9959418ad1ec263f289b85547cb6728a0675f1deadc19cc2b1e088145686f29b` |
| after CALL | 349 | `bf3fad84abda5f1f6381e8aee1e81b910a889367d41a2afe2df8c61e5deb506d` |
| **fork point (below activation)** | **118** | `6942a18dfc0ee15643701b7201c659e721ffe0565d15c510924d5bf4b9a686ed` |
| branch A tip (abandoned) | 349 | `df36336aa59818fbdb07e5a31a2b868477ae72fa1b6d96669c93f2cf9948f1b9` |
| branch B tip (adopted) | 351 | `cd75b9326265c2df65a269b5a8b6663e3e20e41b876cfe9385bfc0110417c39c` |
| after reorg | 351 | `b120099c…` — **back to the empty root** |
| after restart | 351 | `b120099c…` |
| clean reindex from an empty database | 351 | `b120099c…` |

Contract deployed above the boundary:
`961b884280bcf6164a9da3d4266fb1001f98452ae3f0da4f70fe0cb60d966c7c`
(counter, code hash `fa8289fbc0fdb132e57f939033a9971b0ee2990ec49db247aad3f682aa804cc7`,
2,513 bytes).

What this establishes:

- **Pre-activation state is empty** and stays empty across the boundary until a
  protocol transaction actually appears.
- **Crossing the boundary alone changes nothing** — the root at 122 equals the
  root at 119.
- **Rollback can cross below activation safely**: the fork point 118 is below
  the activation height 120, so the rollback traversed the boundary.
- **Stale activated-branch state is removed** — the deploy and the call, and
  every byte of state they wrote, are gone.
- **persisted root == restart root == clean-reindex root**, byte-identical.
- No RocksDB modification of any kind.

### About the regtest activation override

`protocol/v0.toml` pins `regtest_activation_height = 1`, which leaves no blocks
below the boundary and so makes the boundary untestable. The override exists
only to move that boundary **on regtest**:

- It is a **separate field** consulted only when the network is Regtest.
- It is a **hard error** on mainnet or testnet — the workflow proves this with
  a negative test asserting the process refuses to start
  (`ZALKANES_REGTEST_ACTIVATION_HEIGHT is set but the network is … cannot be
  overridden`). It is not a silent no-op.
- It is resolved **before any network I/O**, so a misdirected override fails
  immediately.
- **It does not touch `protocol/v0.toml`.** The protocol manifest hash is
  unchanged: `57178628cebadad21da5e5c6495a7d646a55c0299609ea8939737744ab0b8752`.
- A unit test asserts mainnet and testnet ignore it entirely
  (`activation_override_tests` in `crates/zalkanes-indexer/src/lib.rs`).

## Relationship to the audit candidate

This work is **after** `audit-candidate-v0-rc1` (`79942f50…`), which is
unchanged. The reorg harness is new test machinery plus the regtest-only
activation field; no protocol wire format, ContractId derivation, carrier
semantics, state-root encoding, wasmi configuration, ordering rule or failure
semantic was altered. The candidate's protocol manifest and `Cargo.lock`
digests are unchanged.

## What is still NOT covered

- These are **regtest** reorgs. The public testnet has never been deliberately
  reorganised, and must not be.
- The competing branches are short (2–45 blocks). Zebra finalises below
  1,000 blocks and the wallet's rewind has a 100-block pruning floor, so
  deeper reorgs are untested and remain out of scope.
- The stale-note-row observation above is **open**.
