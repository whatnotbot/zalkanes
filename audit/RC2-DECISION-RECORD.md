# RC2 activation-height decision record

**STATUS: MEASURED AND PROPOSED. NOT COMMITTED. RC2 NOT CREATED.**

No file in `protocol/` has been modified. No tag has been created. No
deployment has happened.

## Why RC2 does not exist yet

`audit/RELEASE-CANDIDATE-LIFECYCLE.md` requires RC2 to be cut from the **merged
readiness state**. PR #2 is currently `BLOCKED` with `reviewDecision:
REVIEW_REQUIRED`. Branch protection requires one approving review and is
enforced for administrators.

That protection is deliberately **not** relaxed and the review is **not**
bypassed. RC2 therefore cannot be frozen yet, and cutting it from an unmerged
branch would produce a candidate whose history does not match `main`.

## Measurement — our own public-testnet Zebra

Taken from the project's own Zebra full validator, not a hosted RPC:

| field | value |
|---|---|
| decision timestamp | **2026-09-12T09:30:43Z** |
| canonical tip height | **4,340,527** |
| canonical tip hash | `0025c70611945e5cbf4314a02374dcbb8fab61c39dc0509ce91181e8a72ea4d7` |
| chain | `test` |
| node | own Zebra v6.3.0 (the trust anchor; no hosted RPC is a dependency) |

## Proposed activation height

| field | value |
|---|---|
| minimum permitted (`tip + 2000`) | 4,342,527 |
| **proposed `H_testnet`** | **4,346,500** |
| lead from the measured tip | **5,973 blocks** (~5.2 days at 75 s/block) |

The proposal deliberately exceeds the 2,000-block floor. The floor is ~2.8
days, and the freeze cannot begin until a human reviews PR #2. A height chosen
tightly against today's tip could silently become unsafe while waiting.

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
