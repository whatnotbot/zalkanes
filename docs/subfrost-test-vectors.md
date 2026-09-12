# SUBFROST AMM v0 — test vectors

Canonical artifact: `test-vectors/subfrost_amm_v0_vectors.json`
(53 vectors: 10 initialize, 10 add, 10 remove, 15 swap, 8 failure).

## Provenance (independence guarantee)

Expected values are produced by `tools/gen_subfrost_vectors.py` using
Python arbitrary-precision integers (`math.isqrt`, big-int `//`). The
generator shares **no code** with the Rust implementation under test.
The generator also emits the Rust table
`crates/zalkanes-dex-core/tests/golden/data.rs` consumed by
`tests/golden_vectors.rs`; the JSON is the canonical artifact.

Never regenerate with different constants without a protocol version
bump: the vectors pin the frozen v0 arithmetic (fees 100/80/20 bps,
MINIMUM_LIQUIDITY 1000, denominator-first swap pricing, floor rounding,
dust rejection for `amount_in < 100`).

## Vector shape

```json
{
  "id": "SWAP-001",
  "op": "swap_exact_in",
  "args": {"reserve_in": ..., "reserve_out": ..., "amount_in": ...},
  "expected": {
    "amount_out": ..., "total_fee": ..., "lp_fee": ..., "protocol_fee": ...,
    "reserve_in": ..., "reserve_out": ..., "protocol_fee_accrued_in": ...
  }
}
```

Failure vectors carry `"expected": {"error": "<DexError name>"}`; the
Rust table stores the numeric code (must match
`zalkanes-dex-core/src/error.rs` exactly).

## Regeneration

```
python3 tools/gen_subfrost_vectors.py .
cargo test -p zalkanes-dex-core --test golden_vectors
```

Any diff in the JSON under an unchanged protocol version is a release
blocker.

## Cross-runtime check

A representative subset of the same shapes is additionally executed
inside the REAL consensus wasmi engine via
`contracts/subfrost-mathcheck` in
`crates/zalkanes-dex-testkit/tests/real_platform.rs`, proving
native == wasm bit-equality on the frozen platform.
