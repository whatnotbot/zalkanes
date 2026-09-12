# RC2 activation-height decision record

**STATUS: HEIGHT CHOSEN AND FROZEN INTO THE CANDIDATE. NOT DEPLOYED.**

`protocol/v0.toml` now carries the new testnet activation height. Nothing has
been deployed to the public testnet and no transaction has been broadcast.

## Decision — re-measured at freeze time

The earlier proposal in this document was explicitly provisional. It was
**re-derived from a fresh measurement** immediately before the freeze, not
carried over.

| field | value |
|---|---|
| decision timestamp | **2026-09-12T11:26:32Z** |
| canonical tip height | **4,340,637** |
| canonical tip hash | `000071aa4d878924ea16a1d2bbf9d3352d982092378805b9ed37727ef5cd56d6` |
| chain | `test` |
| source | our own Zebra v6.3.0 full validator (no hosted RPC is a dependency) |
| **chosen `H_testnet`** | **4,346,500** |
| lead | **5,863 blocks** (~5.1 days at 75 s/block) |
| floor check (`>= 2000`) | satisfied |
| preferred band (5,000–6,000) | satisfied |
| strictly above the tip | satisfied |

The re-derived value coincides with the earlier provisional number because the
tip had advanced only ~110 blocks in the interim; it was recomputed from the
fresh tip, not copied.

## Manifest consequence

| | |
|---|---|
| manifest hash before (RC1) | `57178628cebadad21da5e5c6495a7d646a55c0299609ea8939737744ab0b8752` |
| **manifest hash after (RC2)** | **`06e3df62e5e98a3c276b583e038cbea8d05934e9d2a8e7c00299fec3140bf4bb`** |
| `Cargo.lock` | **byte-identical** — `a8e87a4c852a876390e583e1914931aa396b32b3865ceb336bdcd998aa376199` |
| `MAINNET_ACTIVATION_HEIGHT` | **`None`** — unchanged |

A new manifest hash means a new candidate identity. That is the manifest doing
its job. RC1 stays immutable and is superseded, not replaced in place.

A test now binds the code constants to the manifest values
(`activation_heights_match_the_manifest`), so the two cannot drift apart in a
future change.

## Historical note — why this record previously said RC2 did not exist

`audit/RELEASE-CANDIDATE-LIFECYCLE.md` requires RC2 to be cut from the merged
readiness state. That condition is now met: PR #2 merged as
`9565eba0f450b7fb72726651a1e3416a08f0d2d0`, and this candidate is cut from that
`main`.

**Governance fact, recorded deliberately:** PR #2 merged with **zero submitted
reviews**. The repository's required approving review count was changed from 1
to 0 by the owner, and the owner then merged it. No independent human review of
this work has taken place. This is recorded here because an auditor reading the
candidate's provenance is entitled to know it; it must not be presented as
"reviewed".

## Mandatory re-derivation rule

This height is **provisional**. At freeze time, before editing
`protocol/v0.toml`, re-measure the tip and require:

```
H_testnet - tip_at_freeze >= 2000
```

If that does not hold, **discard this proposal and choose a new height** from a
fresh measurement. Never lower the lead to keep a previously announced number.

The hard floor is unchanged and non-negotiable: `H_testnet` must be **strictly
greater than every block that exists at freeze time**, so no pre-existing
testnet history can be reinterpreted as protocol messages.

## What the freeze will change, and nothing else

1. `protocol/v0.toml` → `testnet_activation_height = <H_testnet>`
2. the matching source constant `TESTNET_ACTIVATION_HEIGHT`
   (`crates/zalkanes-core/src/consensus.rs`)
3. the recorded protocol manifest hash in the audit documents
4. candidate metadata / evidence documents

Explicitly **not** changed: protocol wire encoding, ContractId derivation,
carrier semantics, WASM engine/version/config, fuel schedule, state-root
encoding, execution ordering, failure semantics.

`mainnet_activation_height` stays `"None"`. `Cargo.lock` stays byte-identical
(`a8e87a4c852a876390e583e1914931aa396b32b3865ceb336bdcd998aa376199`) unless a
separately justified security fix requires otherwise.

## Consequence, restated

Editing `testnet_activation_height` changes the manifest hash, which changes
the candidate identity. That is the point of the manifest, not a side effect.
RC1 stays immutable and is superseded, not replaced in place.

## Blocking prerequisites, in order

1. PR #2 reviewed and merged (needs a second account — the owner cannot
   self-approve under `enforce_admins` with one required approval).
2. Re-measure the testnet tip; confirm the lead still holds.
3. Freeze RC2 and **sign** the tag — RC1's unsigned tag must not be repeated.
4. Explicit owner approval before any public-testnet deployment.
