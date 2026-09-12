# Runbook — fresh frozen-RC public-testnet activation

**STATUS: PREPARED, NOT EXECUTED. Requires explicit owner approval.**

No activation height is chosen in this document. Nothing here has been run.

## 0. Why this runbook exists, and the consequence nobody should skip

`audit/TESTNET-ACTIVATION-AUDIT.md` establishes that the current public-testnet
history is **frozen-code acceptance, not a fresh frozen-RC activation**: the
testnet activation height (`4,338,100`) was frozen roughly 16 hours *before*
the protocol was frozen, and before the V4→V5 transaction migration. Some
pre-freeze history above the activation height is therefore read today under
frozen rules.

### The consequence

`testnet_activation_height` lives in `protocol/v0.toml`, and the protocol
manifest hash is **SHA-256 over the exact bytes of that file**. Therefore:

> **Choosing a new testnet activation height necessarily changes the protocol
> manifest hash, and therefore necessarily produces a NEW immutable release
> candidate.** It cannot be done "on top of" `audit-candidate-v0-rc1`.

This is not a defect in RC1 and it does not invalidate the RC1 audit — it is
the manifest doing exactly its job. But it means executing this runbook is a
**release event**, not a deployment tweak:

- `audit-candidate-v0-rc1` (`79942f50…`) stays immutable and stays the audited
  artifact for everything that does not depend on the testnet activation height.
- The activation produces a new candidate — referred to here as **RC-N** — that
  differs from RC1 in exactly one consensus constant.
- The external auditor must be told which of the two they are auditing, and any
  audit performed on RC1 must be re-confirmed against RC-N's single-line diff.

**Do not execute this runbook until the owner has explicitly accepted that it
creates a new candidate.**

## 1. Exact inputs

| item | value |
|---|---|
| Base commit | `79942f5068d91c96529f9b0c6f01bf530e4ee62d` (`audit-candidate-v0-rc1`) plus any merged post-RC1 fixes, named explicitly at execution time |
| Current protocol manifest hash | `57178628cebadad21da5e5c6495a7d646a55c0299609ea8939737744ab0b8752` |
| New protocol manifest hash | **computed at execution time** — `shasum -a 256 protocol/v0.toml` after the edit; record it before deploying anything |
| Cargo.lock digest | `a8e87a4c852a876390e583e1914931aa396b32b3865ceb336bdcd998aa376199` (must not change; this is not a dependency bump) |
| Zebra | `zebrad` **v6.3.0**, pinned, unchanged |
| Rust toolchain | `1.88.0` (`rust-toolchain.toml`), unchanged |
| Network | Zcash **public testnet** (NU6.3 / Ironwood active) |
| Mainnet | `mainnet_activation_height = "None"` — **unchanged by this runbook** |

## 2. Choosing the activation height

Do not commit a height until the owner approves one.

Selection rule:

```
H_activation  =  tip_at_decision_time  +  LEAD
```

| constraint | value | why |
|---|---|---|
| Minimum lead | **≥ 2,000 blocks** (~2.8 days at 75 s testnet blocks) | Every operator must upgrade before activation; an activation that lands before operators upgrade splits the index. |
| Hard floor | Must be **strictly greater than the current testnet tip at the moment of the freeze** | Any height at or below an already-mined block would let *existing* history be reinterpreted as protocol messages. This is the single most important constraint in this document. |
| Recorded | The tip height and hash observed when the decision is made | Proves the floor constraint was met, after the fact. |

Record, at decision time: current tip height, current tip hash, chosen height,
computed lead, and the UTC timestamp.

## 3. Preventing silent reinterpretation of old testnet data

This is the failure mode the whole runbook exists to prevent.

1. **Height floor** (§2): the new activation height is above every block that
   exists when the decision is made, so no pre-existing transaction can be
   parsed as a v0 protocol message under the new activation.
2. **Manifest hash change is the tripwire.** Every node logs and serves
   `protocol_manifest_hash` via `zalkanes_getInfo`. A node still running the old
   manifest is immediately visible and must be treated as out of consensus.
3. **Mandatory clean database.** See §4. The new activation is not compatible
   with a database built under the old one, and this is enforced by procedure,
   not hoped for.
4. **The old deployment is retired, not migrated.** The existing public-testnet
   Zalkanes state (contracts `607a6246…` and `0af32d54…`, roots up to
   `28256d5f…`) belongs to the OLD activation. It is **abandoned**, not carried
   forward. It stays published as historical evidence under the old manifest
   hash and must never be presented as state of the new activation.

### Explicit treatment of the existing pre-freeze deployment

| question | answer |
|---|---|
| Is old state migrated? | **No.** No state migration exists or will be written. |
| Are old contracts redeployed? | Only by deploying them again, as new deployments above the new activation height, receiving new ContractIds. |
| Is the old evidence retracted? | No — it remains valid as *frozen-code acceptance* under manifest `57178628…`. It is superseded, not deleted. |
| Can a node serve both? | **No.** One database, one activation, one manifest hash. |

## 4. Clean database requirements

Non-negotiable, on **every** node:

- Stop the Zalkanes indexer.
- Delete `$ZALKANES_DATA_DIR` entirely (the RocksDB consensus state).
- Do **not** delete or reuse a wallet database across the activation change;
  wallet state is unrelated to the activation height, but the wallet must be
  re-synced against the same Zebra.
- Start with the new binary only. A node that starts on a non-empty database
  built under the old activation is out of consensus by construction.

Zebra's own state is untouched — this is a metaprotocol activation, not a Zcash
network upgrade.

## 5. Node deployment order

1. **Freeze and tag RC-N** (see §7) and build reproducible artifacts. Nothing is
   deployed before the artifacts exist and their digests are recorded.
2. **Node B first** (the non-authoritative comparison node): clean DB, new
   binary, start, confirm `protocol_manifest_hash` equals the new value and that
   it fast-forwards to the activation height with an empty state.
3. **Node A second** (the published endpoint): same procedure. Existing volume
   is **not destroyed** — a new volume/data directory is used, so the old
   deployment remains recoverable for evidence.
4. Both nodes must be running and caught up **before** the activation height is
   reached. Verify remaining lead time explicitly.
5. Do not announce anything until §8 comparison is green.

> The existing public-testnet Zebra service is **not reconfigured** by this
> runbook and no Railway volume is destroyed.

## 6. Rollback plan

The activation height cannot be un-mined, so "rollback" means reverting the
software decision before or shortly after activation.

| situation | action |
|---|---|
| Defect found **before** the activation height is reached | Revert the manifest change, retire RC-N, keep running the old deployment. Nothing on-chain has happened yet. This is a clean abort and is the reason for the ≥2,000-block lead. |
| Defect found **after** activation, no transactions broadcast yet | Stop both nodes. The activation is inert without protocol messages: no ZALK transaction exists above the height, so no state is committed. Fix, re-freeze RC-N+1 with a NEW higher activation height, clean DBs again. |
| Defect found **after** protocol transactions exist | The chain history is permanent. Stop both nodes, publish the defect, and choose a new activation height above the current tip. The abandoned activation's state is discarded, exactly as §3 discards the current one. Do not attempt to patch state in place. |
| Nodes disagree on the root | Stop immediately. Do not "pick a winner". Diagnose with a clean reindex on a third machine before restarting either node. |

## 7. Freeze and tag RC-N

1. Edit `protocol/v0.toml`: `testnet_activation_height = <H>`. Change nothing
   else in the file.
2. Update the source constant to match, and update every audit document that
   quotes the manifest hash.
3. `cargo test --workspace && cargo test --workspace --release`, plus the
   shielded-wallet feature tests, `cargo deny`, wasm contract builds, the ARM64
   determinism workflow, the release-engineering workflow, and the pinned
   live-zebrad regtest workflow. All must be green **before** tagging.
4. Verify `Cargo.lock` digest is unchanged.
5. Record the new manifest hash.
6. Tag **RC-N** — and unlike rc1, **sign it** (`git tag -s`). The unsigned-tag
   finding in `audit/RELEASE-HYGIENE.md` must not be repeated.
7. Build reproducible artifacts, regenerate SBOMs, record all digests.

## 8. Acceptance to run under the new activation

Run in this order, recording evidence at every step (§9).

| # | step | pass condition |
|---|---|---|
| 1 | Both nodes fast-forward to `H` | state empty at `H-1`; root is the empty-state root on both |
| 2 | First activated block | processed exactly once; root recorded |
| 3 | **Fresh contract deployment** — shielded-funded PREPARE | PREPARE mined; carrier outputs created; fee within cap; change returns to Ironwood |
| 4 | **Transparent DEPLOY** spending the carriers | DEPLOY mined; locally derived ContractId equals the indexer's; code hash matches the built WASM byte-for-byte |
| 5 | Initial state read | `get() == 0` at the recorded post-deploy root |
| 6 | **Shielded CALL #1** | mined; execution success; `get() == 1`; `state_root_before` equals step 5's root |
| 7 | **Shielded CALL #2** (spends CALL #1's change note) | mined; `get() == 2`; `state_root_before` equals CALL #1's `state_root_after` |
| 8 | Restart node A | persisted root after restart identical to before restart |
| 9 | Clean reindex on node A from an empty DB | reindex root identical to the persisted root at the same height |
| 10 | **Independent two-node comparison** | node A root == node B root, byte-identical, at the same height, with node B having built its state from an empty database entirely independently |

Fund safety: the acceptance run must not consume the whole test wallet. Budget
~200,000 zat of fees; abort if a planned fee exceeds the 100,000 zat safety cap.

## 9. Exact evidence to save

Into `audit/TESTNET-ACTIVATION-RCN-EVIDENCE.md`, containing **no** seeds,
spending keys, or passphrases:

- Decision record: tip height + hash + UTC timestamp at decision, chosen height, lead.
- RC-N commit SHA, tag name, tag object SHA, **tag signature verification output**.
- New protocol manifest hash; `Cargo.lock` digest; toolchain version; `zebrad --version`.
- Reproducible artifact digests and sizes per target; SBOM file list and digests.
- For every acceptance transaction: txid, mined height, block hash, size, version,
  fee, change amount and destination pool.
- For every step: `state_root_before` → `state_root_after`, and fuel used.
- Derived ContractId vs indexer-recorded ContractId; WASM code hash and byte length.
- Restart root, clean-reindex root, and both nodes' `zalkanes_getInfo` output
  (height, block hash, state root, manifest hash) at the identical height.
- CI run IDs and conclusions for every required check on the RC-N commit.
- Explicit statement that `mainnet_activation_height` is still `"None"`.

## 10. Monitoring during and after activation

| signal | alert condition |
|---|---|
| `protocol_manifest_hash` | any value other than the new hash, on any node |
| node A vs node B `state_root` at equal height | any difference — page immediately, stop both |
| `indexed_height` vs Zebra tip | falling behind by more than a few blocks |
| `/ready` on `PORT+1` | non-200 for more than one block interval |
| Zebra tip | stalled, or reorg depth greater than 2 blocks |
| execution records | any unexpected failure/fuel-exhaustion result |

Run `scripts/compare-state-roots.sh` against both nodes on a schedule for at
least the first 24 hours after activation.

## 11. Sign-off checklist (all must be initialled before execution)

- [ ] Owner accepts that this creates a **new immutable candidate** (RC-N), separate from rc1.
- [ ] Activation height chosen, and proven greater than the tip at decision time.
- [ ] Lead time ≥ 2,000 blocks confirmed.
- [ ] RC-N tagged **and signed**; all required CI green on that exact commit.
- [ ] Reproducible artifacts + SBOMs built and digests recorded.
- [ ] Both nodes deployed on **clean** databases, old volume preserved but retired.
- [ ] Rollback plan (§6) understood and an abort decision-maker named.
- [ ] `mainnet_activation_height` verified still `"None"`.
- [ ] Owner approval recorded, with timestamp.
