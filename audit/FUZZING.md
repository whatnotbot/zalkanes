# Fuzz Summary

## Status

**Executed** (2026-09-12, aarch64-darwin, libFuzzer via cargo-fuzz on
nightly). Five committed targets under `fuzz/`, 412M+ total executions in
the initial bounded campaign (360 s/target) plus post-fix re-runs
(120 s/target). **Two findings, both fixed** with regression seeds
committed under `fuzz/seeds/` and unit regressions in the affected crates:

1. State rollback round-trip violations (re-deploy-then-rollback deleted
   the replaced contract; zero-length storage keys collided with the
   legacy deploy-undo sentinel) — fixed with a versioned `Deploy` undo
   variant carrying the replaced contract.
2. Pinned wasmi 2.0.0 translator panic on a malformed module (node-halt
   class) — contained into a deterministic rejection in
   `validate_module`/`execute`; release builds reject the same input with
   a clean parse error (identical consensus outcome).

Zero unresolved findings. Full per-target statistics: `fuzz/README.md`.

## Planned targets

- protocol decoder (`zalkanes_protocol::parse_op_return`)
- OP_RETURN decoder (`zalkanes_carrier::extract_op_return_data`)
- carrier scriptSig decoder (`zalkanes_carrier::chunk_from_script_sig`)
- chunk reconstruction (`zalkanes_carrier::reconstruct`)
- real Zcash block → Zalkanes extraction (`zalkanes_indexer::parse_block`)
- WASM validator (`zalkanes_runtime::validate_module`)
- WASM ABI decoder / contract return decoder
- storage host functions
- state-root encoder
- rollback journal decoder

## Required properties (per target)

- no panic
- no abort
- no unbounded allocation
- no infinite loop
- no invalid state commit

## Process

Every discovered crash must be minimized (via `cargo fuzz tmin`) and committed
as a permanent regression vector under `test-vectors/fuzz/`.

**This file will be updated with executions, runtime, crashes, and regression
counts once the campaign runs.** Until then it is a placeholder, and the
milestone is not complete.
