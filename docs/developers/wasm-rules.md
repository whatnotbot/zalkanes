# WASM rules for contract authors

The consensus rules for modules are specified in `docs/wasm-consensus.md`
and `protocol/v0.toml` and enforced by `zalkanes_runtime::validate_module`
and the engine configuration. This page is the developer-facing version:
what you can rely on, what gets your module rejected, and why.

## The one rule behind all the others

Every node must compute exactly the same result from the same bytes, input,
and prior state. So a contract may depend only on what the host gives it:
its own storage, the call input, and the block height. Everything else is
either impossible to express in the allowed WASM subset or simply not
provided.

## Things a contract must not depend on

| dependency | why it is unavailable |
|---|---|
| filesystem | no host import provides it; a module importing WASI functions fails validation |
| networking | same |
| wall-clock time | same; the only time-like value is the block height |
| randomness | same; anything "random" must be derived from input or state, and is therefore public and predictable |
| threads | the threads proposal is disabled; there is one instance, one call at a time |
| environment variables, process arguments | no host import |
| native syscalls, FFI | a `wasm32-unknown-unknown` module has no such thing; any unknown import is rejected |
| floating point | **disallowed**: modules containing `f32`/`f64` instructions fail to parse under the consensus engine configuration. Use integers. Rust code that pulls in float formatting or math will not validate |
| unsupported proposals | SIMD, relaxed SIMD, memory64, custom page sizes, wide arithmetic, and a start function are all disabled; bulk memory, reference types, and saturating truncation are outside the v0 profile |
| memory beyond the cap | `memory.grow` past the page limit returns `-1` inside WASM instead of growing |

Also avoid: `std` (it is not available on the target and drags in floats),
`wasm-bindgen`, and any crate that assumes an allocator larger than the page
limit.

## What is checked at validation

`validate_module` runs before a deploy is broadcast (in the CLI) and again on
every node when the DEPLOY is indexed. A module is rejected if:

1. its byte length exceeds `max_code_bytes`;
2. it does not parse under the consensus engine configuration (floats,
   forbidden proposals, start function, malformed bytes);
3. its total function, global, import, or export counts, its initial memory
   pages, or its table size exceed the limits;
4. any import is not one of the six `env` host functions, or is not a
   function import.

A rejected deploy has no effect on state; the transaction that carried it is
still a valid Zcash transaction. The CLI reports the reason before spending
anything.

Numeric values for every limit are generated from the protocol manifest in
[protocol-limits.md](protocol-limits.md).

## Allowed host imports

Exactly these, all in module `env`:

```text
storage_get(key_ptr: i32, key_len: i32, val_ptr: i32) -> i32
storage_set(key_ptr: i32, key_len: i32, val_ptr: i32, val_len: i32) -> i32
storage_delete(key_ptr: i32, key_len: i32) -> i32
input_read(out_ptr: i32, offset: i32, len: i32) -> i32
output_write(ptr: i32, len: i32) -> i32
context_block_height(out_ptr: i32) -> i32
```

Semantics are in [contract-model.md](contract-model.md#host-imports-available-to-a-contract);
the SDK wraps them ([sdk.md](sdk.md)). Importing anything else, including
`contract_call` (declared by the caller example), fails validation with
`unresolvable import env::<name>`.

## Required exports

- `dispatch(opcode: i32, input_len: i32) -> i32`
- `memory`

Extra exports are allowed up to the export limit but nothing calls them.

## Runtime behaviour to plan for

- **Fuel.** Every instruction costs fuel; a call that exhausts its budget
  fails with `fuel exhausted` and its writes are discarded. Budgets are in
  [protocol-limits.md](protocol-limits.md). Keep loops bounded by input or
  state you control.
- **Traps.** `unreachable`, out-of-bounds memory access, integer division by
  zero, and stack overflow are traps. With `panic = "abort"`, a Rust `panic!`
  is a trap. A trap fails the call; it never crashes the node.
- **Non-zero return.** Treated as failure. Use it deliberately for
  validation errors.
- **Host errors are return codes, not traps.** `storage_set` returning `-1`
  does not abort your call; check the result.
- **Storage caps.** Key ≤ 256 bytes, value ≤ 64 KiB, at most 256 distinct
  keys written per call.
- **Determinism across architectures.** The same module must produce the
  same fuel and result on x86_64 and aarch64; CI checks this for the node.
  Do not use `wasm32` intrinsics that are outside the MVP integer subset.

## Testing a module offline

`validate_module` is public in `crates/zalkanes-runtime`; the in-process
testkit ([sdk.md](sdk.md#testing-contracts-in-process)) runs deploy, call,
and view through the real block processor without a Zcash node. The
`wasm_validator` fuzz target in `fuzz/` hammers the validator with random
modules.
