# ADR 0008 — Protocol V1: assets, calls, spawn, events (DEX host ABI)

**Status:** Proposed (implementation on `feat/protocol-v1-dex-abi`; no
activation height chosen)
**Date:** 2026-09-13

## Context

Frozen protocol v0 gives contracts six host imports and executes every
call in a block against pre-block state. That is sufficient for the
audited v0 scope and is consensus-locked. A SUBFROST-style AMM
(`docs/subfrost-amm-v0.md`) requires assets, caller identity,
cross-contract calls, contract spawning, events and sequential
intra-block execution. This ADR specifies those capabilities as
**protocol V1** — a strictly additive version with its own activation
heights and its own manifest.

### Non-negotiable invariant: v0 stays v0

- v0 wire messages (`ZALK` version byte `0x00`) keep their exact frozen
  semantics **forever**, before and after V1 activation: pre-block state
  reads, in-order write application, six-import ABI, v0 fuel rules.
- `protocol/v0.toml` and its manifest hash
  (`06e3df62e5e98a3c276b583e038cbea8d05934e9d2a8e7c00299fec3140bf4bb`)
  are untouched.
- All committed v0 execution vectors must pass unchanged on a V1 node.
- Blocks below the V1 activation height are processed byte-for-byte
  as today; a V1 node replaying pre-V1 history produces identical
  state roots.

## Decision — the 18 consensus items

### 1. AssetId representation

`AssetId = [u8; 32]`. A contract's **native asset** has id equal to its
`ContractId` bytes: `AssetId(contract) := contract.0`. One native asset
per contract in V1. The pool's LP token id is therefore exactly the
pool's contract id.

### 2. Asset balances / custody

A **Holder** is 33 bytes on the wire: tag `0x00` = external account,
`0x01` = contract, followed by the 32-byte id.

External account identity:
`ExternalId = BLAKE2b-256(personal "ZalkAccountId1  ", network_id(1) ‖ compressed_secp256k1_pubkey(33))`.
`ExternalId = [0u8; 32]` is the reserved **Anonymous** account: it can
never hold, send, or receive assets.

The consensus state gains a ledger: `(Holder, AssetId) → u128`
(amount > 0; zero balances are deleted rows). Persisted under the
RocksDB keyspace `l:` ‖ holder(33) ‖ asset(32) → amount (u128 BE).

State-root integration: each ledger row contributes one leaf
`BLAKE2b-256(personal "ZalkAssetLeaf1  ", holder(33) ‖ asset(32) ‖ amount(16 BE))`,
sorted into the same flat leaf set as v0 contract/storage leaves under
the unchanged root personalization `"ZalkStateRoot0  "`. Before V1
activation there are no ledger rows, so every pre-V1 root is
bit-identical to v0's.

### 3. Asset-transfer host calls

New `env` imports (exact signatures in §"Host ABI"): `asset_transfer`
(from the executing contract's custody to any holder), `asset_mint`
(create units of the contract's own native asset), `asset_burn`
(destroy units of the contract's own native asset held in its own
custody). Only a contract can mint or burn, and only its own asset.
Overflow, insufficient balance, zero amounts, or Anonymous recipients
return deterministic error codes and mutate nothing.

### 4. Authenticated caller

Contract-to-contract: the callee's `context_caller` is the calling
contract's `ContractId` (tag `0x01`), assigned by the runtime.

External: a V1 CALL payload may carry an **auth block**
(`pubkey(33) ‖ ecdsa_secp256k1_compact_sig(64)`). The signature is over
`BLAKE2b-256(personal "ZalkCallAuth1   ", network_id(1) ‖ funding_prevout(36) ‖ payload_without_auth)`
where `funding_prevout` is the transaction's first input outpoint
(txid ‖ vout LE u32) — binding the payload to the spending transaction
and making replay impossible. Valid auth ⇒ caller =
`ExternalId(pubkey)`. No auth ⇒ caller = Anonymous; such calls may not
attach assets. Invalid auth ⇒ the message is a protocol violation
(execution records a failure; no state change).

### 5. Cross-contract CALL

`contract_call(target, opcode, input, attached_assets)` executes the
target synchronously with the caller's frame suspended. Attached assets
move from the calling contract's custody into the target's custody
before the target's dispatch runs, and are part of the callee's
`incoming_assets`. Returns the callee's output bytes (≤ 65,536) or a
deterministic error code.

### 6. Nested-call revert semantics

Execution uses a **frame journal**: every frame (top-level message or
nested call) accumulates its own storage writes, ledger movements,
events and spawns in an overlay. On frame success the overlay folds
into the parent frame; on frame failure the overlay is discarded
entirely — storage, ledger, events, spawns — and the parent receives
the error code and continues. Top-level failure discards everything;
the Zcash transaction remains valid, the execution record says
`success=false`, and state is untouched (v0 rule preserved).

### 7. Call depth / recursive fuel

`MAX_CALL_DEPTH = 16` (existing constant, now enforced for real):
opening a 17th frame fails deterministically with `CallDepthExceeded`.
Fuel is **one meter per top-level message**: nested instances draw from
the same remaining budget (wasmi metering per instruction plus the §17
host surcharges). Exhaustion anywhere in the stack aborts the whole
top-level message atomically.

### 8. SPAWN / contract creation

`contract_spawn(code_hash)` instantiates a new contract whose code is
the already-on-chain code identified by `code_hash` (deployed by any
prior DEPLOY or referenced by any live contract). Fresh empty storage,
fresh custody. The spawned contract is not itself re-validated (its
code passed validation at deploy). Spawn fails deterministically if the
code hash is unknown.

### 9. Deterministic ContractId for spawned contracts

`ContractId = BLAKE2b-256(personal "ZalkSpawnId1    ", network_id(1) ‖ txid(32) ‖ spawn_index(u16 BE) ‖ spawner(32) ‖ code_hash(32))`

`txid` is the Zcash transaction whose message is executing;
`spawn_index` counts spawns within that message (0-based, across the
whole frame stack, incremented only for spawns that COMMIT — a reverted
frame's spawns release their indices deterministically because the
counter is journaled with the frame). Collision with a deploy-derived
id is impossible (different personalization).

### 10. Deterministic events / logs

`emit_event(ptr, len)`: ≤ 1,024 bytes per event, ≤ 64 events per
top-level message. Events are journaled with their frame (discarded on
revert) and recorded in the message's `Execution` record in commit
order. Like v0 execution records they are **not** part of the state
root; they are pinned by the V1 execution vectors and exposed via RPC.

### 11. Sequential intra-block execution

Post-V1-activation blocks maintain an **in-block overlay**:

- **V1 messages** read through the overlay: message N+1 sees every
  write committed by messages 1..N of the same block (v0 and v1 alike).
- **V0 messages** keep the frozen v0 rule: reads see only pre-block
  state; their writes still apply at their position in the in-order
  fold (exactly v0's `BTreeMap::insert` behavior today).

A block containing only v0 messages therefore commits bit-identically
to a v0 node.

### 12. Transaction-level atomicity

Unchanged from v0: one protocol message per transaction (first valid
ZALK OP_RETURN); a failed message reverts all of its effects and the
block continues. V1 adds: attached-asset debits from the external
caller are part of the message's atomic effects.

### 13. Block-level ordering

Canonical Zcash block transaction order (as committed by the block),
messages executed in transaction index order. No fee-priority, no
reordering. Identical to v0.

### 14. Reentrancy semantics

The runtime permits reentrant `contract_call` chains up to
`MAX_CALL_DEPTH`; it does NOT impose implicit locks. Contracts are
responsible for their own guards (the SUBFROST pool uses its storage
lock). Rationale: implicit global locks break legitimate composition
(pool → token name view during init) and upstream parity.

### 15. Asset conservation invariant

For every asset `A` and every block: `Σ balances(A) after = Σ before
+ minted_by_A's_contract − burned_by_A's_contract`. Enforced by
construction (the only ledger-mutating operations are transfer — which
is balance-preserving — and mint/burn gated to the native contract) and
pinned by V1 vectors plus a fuzz target.

### 16. Malformed host-call behavior

Out-of-bounds pointers/lengths → error code `-1` returned to the
contract (v0 convention), never a host panic. Malformed enum/tag inputs
(bad holder tag, zero amount where nonzero required, unknown asset) →
specific negative codes (table in `protocol/v1.toml`). A contract that
ignores error codes and returns success still commits only its own
frame — host state is never half-written because every host call is
atomic over the journal.

### 17. Cross-contract fuel accounting

Single meter (§7). Host-call surcharges (consensus constants, in
`protocol/v1.toml`): `asset_transfer/mint/burn` 5,000; `contract_call`
base 10,000; `contract_spawn` 50,000; `emit_event` 1,000 + 10 × len;
`context_*`/`incoming_*` 100; `fuel_consume(n)` charges exactly `n`.
Existing v0 imports keep v0 cost (no surcharge). Budgets
(`MAX_FUEL_PER_CALL/TX/BLOCK`) unchanged.

### 18. Consensus vectors

New immutable vector sets, generate/verify duality like v0:

- `test-vectors/execution/v2.json` — mixed v0+v1 block scenarios:
  sequential visibility, two-swaps-per-block, failed-second-swap
  isolation, nested revert, spawn, events, fuel.
- `test-vectors/protocol/call-v1.json` — payload encode/decode + auth
  sighash + ExternalId derivation.
- `test-vectors/protocol/spawn-id-v1.json` — spawned ContractId
  derivation.
- `test-vectors/state-roots/v1.json` — roots with asset leaves.

## Wire format (V1 messages)

V1 introduces one message type. OP_RETURN payloads stay ≤ 80 bytes, so
the V1 CALL rides in the existing P2SH carrier machinery (ADR-0003,
unchanged):

```
OP_RETURN: "ZALK" ‖ 0x01 ‖ 0x04 ‖ payload_hash(32) ‖ payload_len(u32 BE) ‖ carrier_count(u8)
           (43 bytes; MSG_CALL_V1 = 0x04)
```

Carrier payload (SHA-256 must equal `payload_hash`):

```
contract_id(32) ‖ opcode(u16 BE) ‖ flags(u8) ‖
attached_count(u8 ≤ 4) ‖ attached_count × [asset(32) ‖ amount(u128 BE)] ‖
[recipient Holder(33)   if flags bit1] ‖
input_len(u32 BE) ‖ input ‖
[auth pubkey(33) ‖ sig(64) if flags bit0]
```

`recipient` (default: the caller) is where contract outputs addressed
to the caller land when the caller is Anonymous or shielded-funded.
Attachments require auth (`flags bit0`), and are debited from
`ExternalId(pubkey)`'s ledger balance before dispatch (insufficient
balance ⇒ failed message, no state change). V1 messages are protocol
violations before the V1 activation height.

DEPLOY and v0 CALL/CALL_CARRIER are unchanged and remain valid forever.

## Host ABI (V1 import allowlist extension)

Module `env`, added to the validator allowlist for deploys at/after V1
activation (earlier deploys keep the six-import allowlist):

```
context_self_id(out_ptr) -> i32                        // 32 bytes
context_caller(out_ptr) -> i32                         // 33 bytes
incoming_asset_count() -> i32
incoming_asset_get(index, out_ptr) -> i32              // 48 bytes: asset ‖ amount BE
asset_transfer(to_ptr, asset_ptr, amount_ptr) -> i32   // 33, 32, 16 bytes
asset_mint(to_ptr, amount_ptr) -> i32
asset_burn(amount_ptr) -> i32
emit_event(ptr, len) -> i32
contract_call(target_ptr, opcode, in_ptr, in_len,
              assets_ptr, assets_len, out_ptr, out_cap) -> i32  // ≥0 out len | <0 error
contract_spawn(code_hash_ptr, out_id_ptr) -> i32
fuel_consume(units_i64) -> i32
```

`assets_ptr` packs `n × (asset(32) ‖ amount(16 BE))`. Error codes are
negative; `0`/length = success. The six v0 imports keep their exact v0
behavior.

## Activation mechanism

New per-network constants (all `None`/unset until a future RC freezes
them; regtest gets `Some(2)` for testing only):

```
V1_MAINNET_ACTIVATION_HEIGHT: Option<u32> = None
V1_TESTNET_ACTIVATION_HEIGHT: Option<u32> = None
V1_REGTEST_ACTIVATION_HEIGHT: Option<u32> = Some(2)
```

`protocol/v1.toml` is the V1 manifest (constants, surcharges, error
codes, activation heights); its SHA-256 is the V1 manifest hash.
`zalkanes_getInfo` gains `protocol_versions` (list of
`{version, manifest_hash, activation_height}`) and
`active_protocol_version`; existing fields are unchanged and keep
reporting v0's manifest hash for compatibility.

V1 activation on testnet/mainnet requires the full RC release process
(runbook, clean-DB requirement does NOT apply — V1 is additive and a
V1 node replays v0 history identically; no reindex is needed).

## Test vectors

Listed in §18; all committed under `test-vectors/` with the standard
`ZALKANES_GENERATE_VECTORS=1` regenerate/verify duality and immutable
after the V1 RC freeze.

## Consequences

- The SUBFROST AMM contracts run on the real runtime with only import
  renames (`dex_*` → canonical names) — no logic changes.
- The state root gains a leaf type; pre-V1 roots are unchanged.
- Wallet/CLI must learn V1 CALL construction (carrier payload + auth
  signing) before user-facing testnet V1 flows; consensus does not
  depend on that tooling.
- The v0 audit surface is untouched; V1 is a separate auditable layer.

## Stop condition

If implementation shows any pre-V1 root, vector, or v0 execution
changing, stop and revise this ADR rather than adjusting v0 behavior.
