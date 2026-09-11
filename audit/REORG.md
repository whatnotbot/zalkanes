# Reorg model

## Indexer

The block loop (`crates/zalkanes-cli/src/main.rs`) compares the stored block
hash at its indexed tip against the canonical chain before advancing. On
divergence it walks back one block at a time via
`StateStore::rollback_to`, replaying the per-height undo journal in
descending height order (and reversed within each height), then re-indexes.

The undo journal records, per block: previous values for every storage write,
previous values for every delete, and a dedicated `Deploy` entry carrying the
**replaced contract** (if the id already existed) so a rollback restores it
rather than deleting the id.

> The `Deploy` variant replaced an earlier empty-key `Set` sentinel. The
> sentinel was ambiguous because contracts may legally write zero-length
> storage keys — a rollback across such a write deleted the whole contract.
> Found by the `state_transition` fuzz target; the journal format is now
> versioned (`UNDO_V2_MARKER`) and legacy journals decode compatibly.

Commits are atomic: data writes, undo journal, metadata, the height record
and the state root all land in a single RocksDB write batch, with a
defensive projected-vs-applied root cross-check that fails loudly on
mismatch.

## Wallet

`SqliteShieldedWallet::scan_to_tip` compares the wallet's stored hash at the
common boundary (`min(scanned_tip, canonical_tip)`) with the canonical chain
**before** scanning forward, and rewinds to the common ancestor when they
disagree or when the wallet is ahead of the canonical tip.

> This pre-loop check was added after the reorg matrix caught a real defect:
> divergence was only detected inside the forward loop, which never runs when
> the wallet is at or ahead of the tip — so a same-height tip replacement or
> a shorter canonical branch left the wallet claiming sync on a stale branch.

Rewind uses the canonical `rewind_to_chain_state` and is bounded by the
account birthday: a reorg crossing the birthday is refused loudly (it
requires a restore at an earlier birthday) rather than rewinding into
unscanned history.

## In-flight operations during a reorg

Note reservations are never released on an ambiguous outcome. Broadcast-phase
journal rows are reconciled against our Zebra by txid: mined → `Mined`,
mempool → `BroadcastAccepted`, definitely absent past expiry + a 6-block
reorg margin → `Expired` (locks released); anything else stays ambiguous with
locks held.

## Test coverage

`crates/zalkanes-wallet/src/wallet_reorg_tests.rs` builds real
`zcash_primitives` blocks (chained headers + BIP34 coinbases) into swappable
competing chains and drives the production scan/rewind machinery:

| case | covered |
|---|---|
| linear scan to tip | yes |
| same-height tip replacement | yes |
| shallow reorg (3 blocks) | yes |
| deep reorg (to birthday + 2) | yes |
| reorg crossing the birthday | yes — refused loudly |
| restart immediately before rewind | yes |
| injected fault mid-rescan, restart, converge | yes |
| repeated successive reorgs | yes |
| recovered wallet == clean fresh replay | yes |

Indexer-side rollback/reapply equality is covered by
`crates/zalkanes-state/tests/crash.rs` and the execution-vector rollback case.

## Known gaps

- **Note-level live regtest reorg** — a shielded note received on a removed
  branch disappearing, and a spend rolling back — requires submitting a
  COMPETING chain to a live node. The `live-zebra-regtest` workflow runs a
  real pinned zebrad and exercises live scanning and custody, but
  competing-block construction/submission machinery does not exist in this
  repo yet. Tracked, not claimed.
- Activation-boundary reorg (rollback below the pre-activation fast-forward
  point) has no dedicated test.
