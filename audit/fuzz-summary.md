# Fuzz Summary

## Status

**Not yet executed.** `cargo-fuzz` targets are planned but no long-running fuzz
campaign has been run at the audited commit.

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
