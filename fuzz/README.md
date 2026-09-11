# zalkanes-fuzz

Consensus-critical fuzz targets (cargo-fuzz / libFuzzer, nightly):

- `protocol_frame` — ZALK OP_RETURN parser; canonical re-encode invariant.
- `carrier_decoder` — chunk reconstruction; length + hash enforcement.
- `carrier_scripts` — OP_RETURN extraction, push parsing, carrier scriptSig
  interpretation; no allocation amplification.
- `wasm_validator` — consensus module validation (panic containment mirrored
  from production; see the print-only hook note in the target).
- `state_transition` — commit determinism + rollback round-trip invariants.

Run: `cargo +nightly fuzz run <target> -- -max_total_time=360`
Seed corpora live in `seeds/<target>/` (copy into `corpus/<target>/` or pass
the directory on the command line); `regression-*` seeds are minimized crash
inputs from fixed findings and MUST stay green.

## Campaign log

2026-09-12 (aarch64-darwin, libFuzzer, 360s/target, then 120s re-runs after
fixes):

| target | executions | cov | crashes |
|---|---|---|---|
| protocol_frame   | 292,079,164 | 190  | 0 |
| carrier_decoder  |  85,794,340 |  94  | 0 |
| carrier_scripts  |  34,657,689 | 157  | 0 |
| wasm_validator   | 9,861 → 832,391* | 7227 | 1 → 0 |
| state_transition | 43 → 519,650* | 739 | 1 → 0 |

*after fixing the two findings below and re-running bounded campaigns.

Findings (both fixed, regression seeds committed):
1. `state_transition`: rollback round-trip violation — re-deploying an
   existing ContractId then rolling back deleted the contract instead of
   restoring the replaced one, and zero-length storage keys collided with the
   legacy deploy-undo sentinel. Fixed with a dedicated versioned `Deploy`
   undo variant carrying the replaced contract.
2. `wasm_validator`: pinned wasmi 2.0.0 translator PANICS (debug-profile
   assertion, `translator/func/stack/control.rs`) on a malformed module —
   node-halt class. Production `validate_module` now contains translation
   panics into a deterministic rejection; release builds reject the same
   input with a clean parse error. Consensus outcome (rejection) is identical
   across profiles and architectures.

Unresolved findings: none.
