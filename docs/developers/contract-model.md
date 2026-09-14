# Contract model

How a Zalkanes v0 contract is built, identified, executed, and stored. This
page describes the implementation in `crates/zalkanes-runtime` and
`crates/zalkanes-indexer` as it exists today. Nothing here is aspirational.

## Lifecycle in one picture

```text
Rust crate (no_std, cdylib)
      │  cargo build --target wasm32-unknown-unknown
      ▼
contract.wasm  ── validated ──▶  published on the Zcash chain (DEPLOY)
      │                                 │
      │                                 ▼
      │                        ContractId (32 bytes)
      ▼
every node: on each CALL, instantiate the module, run dispatch(opcode, input_len),
            commit the storage writes atomically if it returns 0, discard them otherwise
```

## Rust to WASM

A contract is a Rust crate compiled for `wasm32-unknown-unknown` as a
`cdylib`. It is `#![no_std]`, provides its own `#[panic_handler]` and global
allocator, and depends on `zalkanes-sdk` for the host calls. The reference
crates under `contracts/` are the template; see [building.md](building.md).

## Deployed code is immutable

DEPLOY publishes the exact WASM bytes on the Zcash chain (inside the
transaction's inputs) together with their SHA-256. Every node reconstructs the
bytes, checks the hash, validates the module, and stores it under a
`ContractId`. There is no upgrade, no self-destruct, and no way to change the
code behind an id. To change behaviour you deploy a new contract and get a new
id. See [deploying.md](deploying.md).

## ContractId

```text
ContractId = BLAKE2b-256(
    personalization = "ZalkContractId0 ",
    network_id (1 byte) || txid (32, internal byte order) || output_index (u16 BE) || code_hash (32)
)
```

`txid` is the ZIP-244 id of the DEPLOY transaction and `output_index` is the
index of its ZALK OP_RETURN output. Because the txid is part of the input,
deploying the same code twice yields two different ids, and the id of a
contract cannot be predicted before its DEPLOY transaction exists. The CLI
prints the id after DEPLOY is mined; the node exposes it via
`zalkanes_getContract`. Test vectors: `test-vectors/protocol/contract-id-v0.json`.

## Deterministic execution

Every node runs the same `wasmi` 2.0.0 engine with the same configuration
(no floats, no SIMD, no memory64, no start function, fuel metering on) over
the same bytes, input, and prior state, so every node computes the same
writes, output, and fuel. Anything that could differ between machines is
either absent from the host ABI or rejected at validation; see
[wasm-rules.md](wasm-rules.md).

## Entrypoint: exactly one export

The runtime looks up one typed export and calls it:

```rust
#[no_mangle]
pub extern "C" fn dispatch(opcode: i32, input_len: i32) -> i32
```

- `opcode`: the `u16` method selector from the CALL message, widened to `i32`.
  How opcodes map to behaviour is entirely the contract's choice; the
  convention in the examples is a `match opcode as u16 { … }`.
- `input_len`: the length in bytes of the call input. The bytes themselves
  are fetched with `input_read` (SDK: `read_input()`).
- Return value: **`0` means success.** Any other value aborts the call as a
  trap with reason `dispatch returned error code N`. Nothing is committed.

The module must also export its linear memory under the name `memory`
(Rust's wasm target does this by default). Host functions that cannot find
that export return `-1`. A module without a `dispatch` export traps with
`missing 'dispatch' export`.

## Input and output bytes

Input is an opaque byte string chosen by the caller. The CLI takes it as hex.
Output is an opaque byte string the contract sets with `output_write`
(SDK: `write_output`). The last write wins. On success the output becomes the
`return_data` of the execution record (for CALL) or `output_hex` (for VIEW).
On failure the output is empty.

There is no ABI encoder in v0. Each contract documents its own encoding; the
examples use big-endian fixed-width integers (`u64`, `u128`) and
length-prefixed byte strings. Limits on input and output size are in
[protocol-limits.md](protocol-limits.md).

## Host imports available to a contract

The linker provides exactly six functions, all in module `env`. A module that
imports anything else fails validation and cannot be deployed.

| import | signature | behaviour |
|---|---|---|
| `storage_get` | `(key_ptr, key_len, val_ptr) -> i32` | copies the value into memory at `val_ptr`; returns its length, or `-1` if absent or out of bounds |
| `storage_set` | `(key_ptr, key_len, val_ptr, val_len) -> i32` | `0` on success; `-1` if the key or value exceeds its limit, the write cap is reached, or a range is out of bounds |
| `storage_delete` | `(key_ptr, key_len) -> i32` | `0` on success; `-1` on limit or bounds failure |
| `input_read` | `(out_ptr, offset, len) -> i32` | copies up to `len` input bytes from `offset`; returns bytes written (`0` past the end) |
| `output_write` | `(ptr, len) -> i32` | sets the return data; `0` on success, `-1` if too large or out of bounds |
| `context_block_height` | `(out_ptr) -> i32` | writes the current block height as a big-endian `u32`; `0` on success |

Not available in v0, and therefore not something a contract can rely on: the
calling transaction's id, the contract's own id, the identity of the caller,
any Zcash value transfer, and calls into other contracts. `docs/wasm-consensus.md`
lists a wider ABI that was planned; the six functions above are what the
frozen v0 runtime implements.

## Persistent key/value storage

Each contract has a private byte-keyed, byte-valued store. Keys are up to 256
bytes, values up to 64 KiB. Storage is read through to the node's database
and written to a per-call overlay; reads within the call see the overlay.
A call may write at most 256 distinct keys (a rewrite of the same key does
not count twice); the 257th distinct key makes `storage_set` return `-1`.

## Atomic state changes

Writes are buffered during the call. If `dispatch` returns `0`, the buffer is
applied; otherwise it is discarded. Then all executions in a block are
committed to RocksDB in one atomic batch together with the new state root and
an undo journal, so a crash never leaves a half-applied block.

## Failure: traps and fuel exhaustion

A call fails when any of these happen:

| cause | recorded error |
|---|---|
| `dispatch` returns non-zero | `dispatch returned error code N` |
| a WASM trap (`unreachable`, out-of-bounds memory access, stack overflow, …) | the engine's trap message |
| fuel runs out | `fuel exhausted` |
| the module fails to instantiate | `instantiation error: …` |
| the contract id is unknown | `contract not found` |

In every case the storage writes of that call are dropped and the output is
empty. The **Zcash transaction is still valid and still mined**; a failed
contract execution never invalidates the transaction that carried it. The
fee is spent either way, and the execution record says `"success":false`
with the reason. See [calling-contracts.md](calling-contracts.md).

## Fuel

Every instruction consumes fuel per the `wasmi` 2.0.0 metering table. A call
starts with the smaller of the per-call limit and what remains of the
per-transaction and per-block budgets (values in
[protocol-limits.md](protocol-limits.md)). Fuel used is recorded in every
execution record and view result; the counter's `increment` costs 3,936.
Fuel is a hard cap on work, not a market: there is no gas price and no
Zalkanes-level payment for fuel. The only cost of a CALL is the Zcash
transaction fee (ZIP-317).

## Everything is public

Code, calldata, and state are cleartext on the Zcash chain and in every
node's database. Anyone can read a contract's storage with a VIEW, fetch its
bytes with `zalkanes_getCode`, and replay every execution. Shielded funding
changes none of this; see [wallet-funding.md](wallet-funding.md).

## Block context

The only context a contract can read is the block height of the block being
executed (`context_block_height`). During a VIEW it is the node's current
indexed height. Contracts must not assume anything about wall-clock time.
