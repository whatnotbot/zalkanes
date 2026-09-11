# Audit of the frozen-RC testnet activation claim

This document exists because an earlier revision of the evidence risked
implying that the public-testnet history *is* a fresh frozen-RC activation.
It is not. The distinction is recorded here rather than relabelled.

## Chronology (from `git log`)

| when | commit | event |
|---|---|---|
| 2026-09-10 22:05 | `612f9b8` | `TESTNET_ACTIVATION_HEIGHT = 4,338,100` frozen |
| 2026-09-11 02:14 | `920f8a8` | PREPARE/DEPLOY/CALL migrated to **V5 + ZIP-244** |
| 2026-09-11 14:14 | `af38343` | protocol v0 frozen (`FROZEN-RC`, tag `zalkanes-v0.1.0-rc1`) |
| 2026-09-11 17:00+ | this hardening phase | live shielded acceptance, at heights ≥ 4,339,397 |

**The activation height was frozen ~16 hours BEFORE the protocol was**, and
before the V4→V5 transaction migration.

## The four questions, answered

### 1. Exact activation height
`4,338,100` (testnet). `MAINNET_ACTIVATION_HEIGHT = None`.

### 2. Exact protocol manifest hash
`57178628cebadad21da5e5c6495a7d646a55c0299609ea8939737744ab0b8752`.
`protocol/v0.toml` has not been modified since `af38343`, so the frozen
manifest at the audit candidate is byte-identical to the manifest at the
freeze commit.

### 3. Do activation semantics at that height already equal the current RC?
**Yes, for the indexer's interpretation.** The activation height is a frozen
constant; the rules applied from it are whatever the current binary
implements. Crucially this was *verified empirically*, not assumed: an
independent node with an **empty database** re-indexed the entire chain from
activation using the current code and reproduced the other node's state root
byte-for-byte at height 4,339,534
(`28256d5f65020dddcdd5befc70f4d80f25de605232765980c3f87c1136e86f95`).

So the live state **is** the result of applying exactly the frozen rules to
the full post-activation history. There is no stale pre-freeze state being
carried forward.

### 4. Is pre-freeze history being reinterpreted?
**Yes — and this is the honest caveat.** Heights between 4,338,100 and the
freeze contain transactions produced by *pre-freeze tooling* (V4-era
construction, the original `607a6246…` counter deployment and its first two
calls). Those transactions are read today under frozen rules.

Why this is nonetheless well-defined:

- The carrier and OP_RETURN formats the indexer parses are **transaction-
  version independent** — the parser reads scriptSigs and OP_RETURN outputs,
  not the tx version. A V4-constructed carrier is parsed identically by the
  frozen code.
- Determinism is proven by construction: two independent implementations of
  the state hashing agree, and two independent nodes replaying the same
  history agree byte-for-byte.

Why it is still not a *fresh* frozen-RC activation:

- The activation height predates the freeze, so the frozen protocol was never
  the only protocol that ever ran above it.
- Part of the history was produced by tooling that no longer exists in the
  tree.

## Verdict

| question | answer |
|---|---|
| current activation exact frozen RC | **NO** — height frozen before the protocol froze |
| current state reconstructed under exactly frozen semantics | **YES** — proven by independent empty-DB resync |
| pre-freeze history reinterpreted | **YES** — but format-compatible and deterministic |
| fresh RC activation required for a clean claim | **YES** |
| fresh activation performed | **NO — requires explicit approval** |

The live evidence in `LIVE-ACCEPTANCE-EVIDENCE.md` is therefore accurate as
*frozen-code acceptance on public testnet* and must **not** be described as a
fresh frozen-RC activation.

## Fresh testnet RC activation plan (NOT executed)

Executing this changes a public network parameter irreversibly and is held
pending explicit approval.

1. **Choose a future height** above the then-current tip with margin
   (previous freeze used tip + ~84 blocks), so no pre-existing history can be
   reinterpreted as protocol messages.
2. **Set** `TESTNET_ACTIVATION_HEIGHT` to that height and update
   `protocol/v0.toml`. This changes the protocol manifest hash — that is
   expected and is the point.
3. **Re-freeze**: tag a new RC; the manifest hash in every audit document
   must be updated to match.
4. **Fresh state**: every node starts from an EMPTY database; no state is
   carried across the activation change.
5. **Repeat acceptance under the new activation**: deploy the counter,
   transparent CALL, shielded CALL, shielded-funded PREPARE → transparent
   DEPLOY, restart persistence, and a clean reindex root comparison.
6. **Two-node equality** at the new activation, from empty databases.
7. **Publish** the new evidence, superseding `LIVE-ACCEPTANCE-EVIDENCE.md`.

### Cost and prerequisites

- New testnet funds for the acceptance transactions (the current wallet
  retains ≈ 9.75 M zat, likely sufficient).
- Coordination of every operator, since the manifest hash changes.
- Approval to change a public activation parameter.

Until then, `MAINNET_ACTIVATION_HEIGHT` remains `None` and mainnet
activation remains gated on external audit regardless of this item.
