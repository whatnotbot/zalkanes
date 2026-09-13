# SUBFROST/Alkanes AMM — upstream parity notes (v0)

Status: reference document for the `feat/subfrost-amm-v0` branch.
This document records exactly what upstream source was inspected, what
behavior the Zalkanes port preserves, and every intentional deviation.

> This implementation ports the AMM architecture of SUBFROST/Alkanes to
> Zalkanes. It does not implement SUBFROST's FROST Bitcoin-custody
> subsystem.

## Upstream source inspected

| Repo | Commit | Role |
|---|---|---|
| `kungfuflex/alkanes-rs` | `62511e9371a3f9e448841140c51cfe428cfcb955` | platform, CLI AMM client, `MintableToken`, `AlkaneId` ordering |
| `Oyl-Wallet/oyl-amm` (the `kungfuflex/oyl-amm` link in the alkanes-rs README redirects here) | `80b04f3daa3e6f1743c0dfbcd6ebdf71c4169c9c` | the AMM itself |

Key upstream files:

- Pool logic: `oyl-amm/alkanes/alkanes-runtime-pool/src/lib.rs`
- Pool dispatch: `oyl-amm/alkanes/pool/src/lib.rs`
- Factory logic: `oyl-amm/alkanes/alkanes-runtime-factory/src/lib.rs`
- Factory dispatch: `oyl-amm/alkanes/factory/src/lib.rs`
- Math library: `oyl-amm/alkanes/oylswap-library/src/lib.rs`
- LP mint/supply: `alkanes-rs/crates/alkanes-std-factory-support/src/lib.rs`
- Tests: `oyl-amm/src/tests/{amm,add_liquidity,burn,swap_tests,fees,attacks,precision_loss}.rs`

## Discovered upstream facts (source evidence)

### Pool opcodes

```
0   InitPool { alkane_a, alkane_b, factory }
1   AddLiquidity
2   WithdrawAndBurn
3   Swap { amount_0_out, amount_1_out, to, data }   (low-level, optimistic, flash-capable)
10  CollectFees (factory-only)
20  GetTotalFee / 21 SetTotalFee (factory-only)
50  ForwardIncoming
97  GetReserves            -> two LE u128
98  GetPriceCumulativeLast -> two LE U256 (Q128 TWAP)
99  GetName                -> "{name_a} / {name_b} LP"
999 PoolDetails            -> PoolInfo bytes
```

### Fees

`oylswap-library/src/lib.rs`:

```
DEFAULT_TOTAL_FEE_AMOUNT_PER_1000 = 10    (1.0% total)
PROTOCOL_FEE_AMOUNT_PER_1000      = 2     (0.2% protocol share)
```

Router pricing (`get_amount_out`) is denominator-first, floor:

```
amount_in_with_fee = (1000 - fee) * amount_in
amount_out = floor(amount_in_with_fee * reserve_out
                   / (1000 * reserve_in + amount_in_with_fee))
```

Pool-side enforcement is a Uniswap-V2-style K check on fee-adjusted
balances. Protocol fees are NOT skimmed per swap; they are realized as
LP-token dilution on liquidity events (`_mint_fee`, √k growth, ≈1/5 of
growth with default fees) and accumulate as `claimable_fees` until the
factory calls `CollectFees`.

### Initial liquidity

`alkanes-runtime-pool/src/lib.rs`:

```
MINIMUM_LIQUIDITY = 1000
initial LP = floor(sqrt(amount_a * amount_b)) - 1000
```

The 1000 units are locked by inflating `total_supply` without minting
them to anyone. Sub-minimum initialization reverts (checked_sub).

### Mint / burn

```
mint:  lp = min(floor(a_in * S / reserve_a), floor(b_in * S / reserve_b));  lp == 0 -> revert
burn:  amount_i = floor(lp * reserve_i / S);  amount_a == 0 || amount_b == 0 -> revert
```

Both run inside a storage `/lock` reentrancy guard (`Err("LOCKED")`).

### LP identity

The pool IS its own LP token: `mint` pays `AlkaneTransfer { id: context.myself, value }`.
LP AlkaneId == pool contract AlkaneId.

### Factory

- Permissionless `CreateNewPool` (opcode 1); owner-gated fee/collect ops exist upstream.
- Canonical ordering: derived `Ord` on `AlkaneId {block, tx}`; `sort_alkanes` puts min first.
- Duplicate prevention: storage pointer `/pools/<a>/<b>` (sorted key) must be empty.
- Registry: `/all_pools/<index>` + `/all_pools_length`; enumeration in creation order.
- Pools spawned as beacon proxies via cellpack factory reserved IDs.
- Deadline semantics: `deadline != 0 && height > deadline -> "EXPIRED deadline"`.

### Numeric discipline

All amounts `u128`; intermediates `ruint` U256; Babylonian floor sqrt;
no floating point; `checked_*` + `TryInto<u128>` on narrowing.

## What the Zalkanes port preserves (v0)

- Constant-product exact-input pricing, denominator-first, floor — bit-identical
  to upstream `get_amount_out` for the frozen 1.00% fee
  (bps 9900/10000 == per-1000 990/1000, floor-for-floor).
- 1.00% total / 0.80% LP / 0.20% protocol economic split.
- `MINIMUM_LIQUIDITY = 1000`, locked by supply inflation with no holder.
- Initial LP `floor(sqrt(a*b)) - 1000`, wide (256-bit) product.
- Mint `min(...)` rule; burn floor rule; zero-output burn rejected.
- Add-liquidity optimal-amount branch rule (token1-optimal first).
- LP asset identity == pool contract identity.
- Pool opcodes 0/1/2/3 and views 97/99/999; factory create/find/list/count.
- Storage `lock` reentrancy guard on every mutating pool op.
- Deadline: `expiry_height == 0` disables; else `height > expiry -> Expired`.
- Permissionless pool creation; duplicate canonical pair rejected;
  registration only after successful pool initialization; enumeration in
  creation order.

## Intentional deviations for Zalkanes

| # | Deviation | Reason |
|---|---|---|
| 1 | Fee constants expressed in bps (10000 denominator) instead of per-1000 | Zalkanes DEX spec; numerically identical for the frozen 1.00% fee |
| 2 | Protocol fee is an explicit per-swap skim of the input asset (`floor(in * 20 / 10000)`), tracked in `protocol_fees0/1` accumulators, excluded from reserves | Required explicit-accounting model for this milestone; upstream's √k LP-dilution model needs owner-auth CollectFees machinery that v0 excludes. Fee *collection* is not implemented in v0 (no admin keys). |
| 3 | Pool opcode 3 is a safe exact-input swap (`min_amount_out`, `expiry_height`), not the upstream low-level optimistic swap; no flash swaps, no exact-output, no router/multi-hop | v0 scope: direct pool swaps only |
| 4 | Swaps with `floor(amount_in * 1%) == 0` (i.e. `amount_in < 100`) are rejected (`ZeroAmount`) instead of priced | Dust guard; keeps reported fee accounting exact |
| 5 | Opcode 98 (cumulative price / TWAP) returns deterministic `Unsupported` (code 21) | Upstream TWAP uses Bitcoin header timestamps; no fake TWAP in v0 |
| 6 | Desired add-liquidity amounts are the assets physically attached to the call; calldata carries only `min_lp_out` + `expiry_height` | Zalkanes call model; prevents desired/attached mismatch |
| 7 | Numeric wire encoding is big-endian (Zalkanes platform convention); upstream returns LE u128s | Platform consistency; documented in the ABI |
| 8 | Canonical pair ordering is byte-wise `Ord` on the 32-byte `AssetId` | Zalkanes has no `{block, tx}` AlkaneId; byte order of the canonical AssetId encoding is the analogue |
| 9 | Pair key is `"zalkanes-subfrost-pair-v0" ‖ token0 ‖ token1` (plain domain-separated concatenation, no hash) | Fixed-width fields make the key collision-free; avoids inventing a new hash convention |
| 10 | No owner/admin surface at all: no SetTotalFee, no CollectFees, no auth tokens; the fee is a frozen constant | v0 milestone excludes governance/admin; upstream owner ops documented for future versions |
| 11 | No Bitcoin carriers: no Runestone/Protorune/Protostone, no shadow vouts, no cellpacks, no beacon proxies | Zalkanes-native abstractions only |
| 12 | Pool init takes an explicit `provider` (forwarded by the factory) instead of alkanes response-chaining | Zalkanes call model has no incoming-parcel response chaining |

## Parity claims discipline

Golden vectors (`test-vectors/subfrost_amm_v0_vectors.json`) pin the
frozen arithmetic; they are generated by an independent Python
big-integer reference (`tools/gen_subfrost_vectors.py`), not by the Rust
code under test. No exact-parity claim is made for any behavior not
listed under "preserves" above.
