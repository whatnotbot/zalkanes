# RC2 testnet activation — evidence (pre-activation phase)

Per `audit/TESTNET-ACTIVATION-RUNBOOK.md` §9. This file records the
executed pre-activation steps. The post-activation acceptance section is
appended after height 4,346,500.

Contains NO seeds, spending keys, or passphrases.

## Owner authorization

Track-A execution order received 2026-09-13 with the exact immutable
identity below ("SHIP ZALKANES V0 TESTNET"). This constitutes the
runbook's required explicit owner approval.

## RC2 identity (verified live)

| item | value |
|---|---|
| tag | `audit-candidate-v0-rc2` (annotated, tag object `0faf702e…`) |
| tag signature | GOOD — SSH ED25519 `SHA256:Z1Ylo2qnF3Q9VzaHO/gKjFyzN3FpEqwtKvNZ3Z63Mvo` (owner key) |
| commit | `ed15911d3ac39a645fd9dc064f85ce879cb729f2` |
| protocol manifest (recomputed from tag) | `06e3df62e5e98a3c276b583e038cbea8d05934e9d2a8e7c00299fec3140bf4bb` — MATCHES the ordered identity |
| activation height | **4,346,500** (`TESTNET_ACTIVATION_HEIGHT` and `protocol/v0.toml` agree) |
| `MAINNET_ACTIVATION_HEIGHT` | `None` (verified at the tag) |

## Decision-time lead re-check (runbook §2, re-queried at execution)

| item | value |
|---|---|
| queried | 2026-09-12T18:0xZ UTC (execution start) via own stack |
| own Zebra canonical tip | **4,340,913** |
| remaining lead | **5,587 blocks** ≥ 2,000 → **PROCEED** (no RC3 required) |

## Node deployment (runbook §4/§5 — clean databases)

The previous public endpoint (`zalkanes-testnet`, old volume, state root
`28256d5f…` of the superseded activation) was left untouched as
historical evidence. Two NEW independent RC2 nodes were deployed from
the exact tag source (Docker build of the `audit-candidate-v0-rc2`
checkout), each with a brand-new volume/database, both against the same
own Zebra validator (`zebra-testnet`, v6.3.0):

| node | service | endpoint | volume |
|---|---|---|---|
| A2 | `zalkanes-testnet-a2` | `https://zalkanes-testnet-a2-production.up.railway.app` | `zalkanes-testnet-a2-volume` (new) |
| B | `zalkanes-testnet-b` | `https://zalkanes-testnet-b-production.up.railway.app` | `zalkanes-testnet-b-volume` (new) |

A third clean-DB node was run locally from a from-source RC2 build
(macOS aarch64 binary SHA-256
`56ce4b3fd1366e384a1ed893932a6b755ae6df73d89f22ebf44272c0b5f9dc5a`)
against the same Zebra via its TCP proxy, and reported identical
manifest/height/hash/root.

## Pre-activation verification (runbook §8 step 1, executed early)

Same-height snapshot, both cloud nodes, byte-identical:

```
indexed_height:        4,342,037  (== own Zebra tip)
indexed_block_hash:    00455056abb64ef328c84dc9d0b6eeb48485cded0b3e7fc4316fe3feb6241f7a
state_root:            b120099c167da673588b15aa827c3bbd9339a9a2d934b0d20c99cebe69f8ffe6
                       (the EMPTY state root — clean pre-activation state)
protocol_manifest_hash 06e3df62e5e98a3c276b583e038cbea8d05934e9d2a8e7c00299fec3140bf4bb (RC2)
```

Both nodes log `fast-forwarded below activation height` for every block,
confirming no pre-activation state interpretation.

## Operational finding (recorded; no RC2 code change)

`zalkanes node serve` spawns its indexing loop as a task that dies
permanently on a transient Zebra RPC race (`Provided index is greater
than the current tip`) while the RPC server keeps serving stale state.
Observed once on each new node during initial catch-up; recovery =
process restart (state unaffected; deterministic resume). Mitigation
deployed per runbook §10 monitoring: a watchdog compares
`indexed_height` vs `chain_tip_height` on both nodes every 15 minutes
and restarts a node lagging > 5 blocks. Recommend a serve-loop retry in
the next non-consensus RC.

## Wallet

The existing testnet wallet (funded, ≈9,755,000 zat shielded +
50,000 zat transparent after prior acceptance) remains available for the
post-activation acceptance and will be re-synced against the same Zebra
(runbook §4: wallet is not deleted across the activation change). The
acceptance budget (~200,000 zat, 100,000 zat/tx fee cap) is covered; no
new faucet funding is required.

## Pending (blocked on chain time only)

- Activation at height **4,346,500** (~4.4 days from the snapshot above
  at 75 s/block; ETA ≈ 2026-09-17 UTC).
- Fresh acceptance per runbook §8 steps 2–10 (shielded PREPARE →
  transparent DEPLOY → `get()==0` → shielded CALL ×2 → restart →
  clean reindex → two-node comparison), evidence appended here.
- Switching the published production endpoint to an RC2 clean-DB
  node/data-dir (deliberately left to the operator: production-service
  mutation was excluded from this run).
