# SUBFROST AMM v0 — security review record (spec §58)

Manual review of the branch `feat/subfrost-amm-v0`, item by item.
"Test" references name the enforcing automated test.

| Area | Finding | Enforcement |
|---|---|---|
| Overflow | All arithmetic checked; 256-bit intermediates for products (`wide.rs`); quotient overflow errors, never wraps | MATH-054, PROP-019, FAIL-008, atomic_overflow, fuzz targets |
| Rounding | Explicit floor everywhere; dust swaps (`in < 100`) rejected; zero-output burns rejected; add-liquidity floors pinned by golden vectors | MATH-023/032/042/049..051, 53 golden vectors (independent Python reference) |
| Fee splitting | `lp + protocol == total` exact identity; boundary cases (in=100 → lp 0 / protocol 1) pinned | MATH-048..051, PROP-019, POOL-029 |
| Protocol-fee extraction | No extraction exists in v0 (no admin, no treasury op); accumulators monotone | FACTORY-012, PROP-008 |
| LP inflation | Supply only changes via initialize/add/remove; locked minimum held by nobody; conservation `circulating + 1000 == supply` | PROP-014, POOL-007/008, malicious_reentrant test |
| Initial-liquidity attack | `MINIMUM_LIQUIDITY = 1000` permanently locked via supply inflation (upstream parity), raising the cost of share-price manipulation | MATH-010/011, POOL-008, MATH-033 |
| Donation / reserve desync | Reserves are accounting state; donations cannot move price or redemption; surplus inert (no skim in v0, documented) | donation_does_not_corrupt_pricing |
| Duplicate-pool races | Registry keyed by canonical pair key; check-then-register inside one atomic call; sequential deterministic execution model | FACTORY-004, PROP-003, atomic_duplicate_pool |
| Reentrancy | Storage lock on all mutating ops; initialize writes full state + takes lock BEFORE outbound name calls (fixed during development — see below) | POOL-031, malicious_reentrant_token_cannot_corrupt_init |
| Nested calls | Nested failure reverts nested effects only; outer failure reverts everything; depth capped; fuel shared per call chain | pool_spawner_trait_is_narrow_and_atomic, malicious_fuel_burner_fails_creation_atomically |
| Partial transfers | Ledger transfers all-or-nothing; incoming assets credited pre-dispatch and restored on failure | atomicity suite (11 cases), fuzz dex_swap conservation |
| Malformed calldata | Strict-length `Reader` with mandatory `finish()`; trailing bytes rejected; decoders total | POOL-032, fuzz_style_decoder_robustness, dex_*_calldata fuzz targets |
| Storage-key collision | Fixed-prefix keys (`pair/`, `pool/`, `poolpair/`) with fixed-width suffixes; pair key domain-separated + fixed-width → collision-free | construction; PROP-003 |
| ContractId/AssetId confusion | Distinct types; `AssetId::of_contract` is the only bridge; LP identity test pins bytes equality | POOL-010 |
| Integer truncation | No `as` narrowing on values; `TryFrom`/checked conversions; the one registry-index cast is bounds-guarded (`MAX_POOLS`) | code review; FACTORY-010 |
| Expiry handling | Height from execution context only; `0` disables (upstream parity); `height > expiry` rejects | POOL-015/019/024, atomic_expired_call |
| Slippage handling | `min_lp_out` / `min0`/`min1` / `min_amount_out` enforced after exact quote | POOL-014/017/018/023, atomic_min_* |
| Out-of-fuel | Deterministic OutOfFuel; entire call reverts including partial writes | POOL-034/035, atomic_out_of_fuel |
| Rollback | Snapshot-based; reorg == clean reindex root | e2e_reorg_rollback_replay |
| Restart | serialize/restore preserves root; restored chain functional | e2e_restart_after_every_block |
| Factory half-initialization | Registration strictly after successful pool init; spawn failure reverts | FACTORY-008/009 |
| Malicious tokens | Transfers are passive (no code on transfer — documented §60); name-resolution call-outs are the only surface: reentering/trapping/garbage/fuel-burning tokens all contained | security.rs suite |

## Issue found and fixed during this review cycle

**Half-initialized pool reentry via token name resolution.** The first
implementation of `initialize` resolved constituent token names (an
outbound contract call) *before* setting the `init` flag; a malicious
token could re-enter `initialize`/`add_liquidity` against a
half-written pool. Fixed by writing the complete state and taking the
reentrancy lock before any outbound call
(`contracts/subfrost-pool/src/lib.rs`, `initialize`). Regression tests:
`malicious_reentrant_token_cannot_corrupt_init` (both reenter modes,
with an on-chain `attack = blocked` marker).

## Residual risks (documented, out of v0 scope)

1. **UB-5 intra-block semantics**: on the frozen platform, two calls in
   one block read pre-block state (last write wins) — disqualifying for
   an AMM and unfixable at application level. The DEX testkit models the
   required sequential semantics; the platform change is an upstream
   ADR.
2. **Donation surplus is unrecoverable** (no skim); acceptable for v0,
   revisit with protocol-fee collection.
3. **Test-token mint is permissionless** by design; it must never ship
   beyond test networks (named and documented accordingly).
4. **Fuel schedule in the DEX testkit is a model**, not the wasmi
   metering table; real metering evidence comes from the mathcheck
   contract on consensus wasmi.
