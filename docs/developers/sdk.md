# SDK reference: `zalkanes-sdk`

`crates/zalkanes-sdk` is a small `#![no_std]` crate that wraps the six v0 host
imports and adds two integer helpers. This page documents every public item
from the source (`crates/zalkanes-sdk/src/lib.rs`). There are no macros, no
derive helpers, and no response types; the contract exports `dispatch` itself.

Add it to a contract crate as a path dependency:

```toml
[dependencies]
zalkanes-sdk = { path = "../../crates/zalkanes-sdk" }
```

## Storage

### `get(key: &[u8]) -> Option<Vec<u8>>`

Reads the value stored under `key` for the calling contract.

- Returns `None` when the key is absent, when the key is longer than the
  storage key limit, or when the host reports any error.
- Allocates a 64 KiB buffer for the copy (the maximum value size), so it is
  cheap in fuel but not free in memory.
- Reads see writes made earlier in the same call.

```rust
let count = sdk::get(b"counter").map(|b| sdk::bytes_to_u64(&b)).unwrap_or(0);
```

### `set(key: &[u8], value: &[u8]) -> bool`

Writes `value` under `key`. Returns `true` on success and `false` when the
host refused the write: key longer than 256 bytes, value longer than 64 KiB,
or the per-call cap of 256 distinct written keys already reached. The write
only becomes visible to other calls if `dispatch` returns `0`.

```rust
if !sdk::set(b"counter", &sdk::u64_to_bytes(next)) {
    return -1; // surface the failure as a trap instead of ignoring it
}
```

### `delete(key: &[u8]) -> bool`

Removes `key`. Returns `true` on success. Deleting an absent key succeeds.
Counts toward the same 256-key write cap as `set`.

## Input and output

### `read_input() -> Vec<u8>`

Returns the call input bytes (up to 64 KiB; the CLI currently sends at most
38 bytes inline, see [calling-contracts.md](calling-contracts.md)). Returns an
empty vector when there is no input.

```rust
let input = sdk::read_input();
if input.len() < 8 { return -1; }
let v = sdk::bytes_to_u64(&input[..8]);
```

### `write_output(data: &[u8]) -> bool`

Sets the call's return data. Returns `false` if `data` exceeds the return
data limit or the copy fails. Calling it twice replaces the earlier output.
The output is only delivered if `dispatch` returns `0`.

## Context

### `block_height() -> u32`

The height of the Zcash block being executed (during a VIEW, the node's
indexed height). This is the only environmental input a v0 contract has.

## Integer helpers

### `u64_to_bytes(v: u64) -> [u8; 8]`

Big-endian encoding, the convention used throughout the examples.

### `bytes_to_u64(b: &[u8]) -> u64`

Decodes the first 8 bytes big-endian. **Returns `0` if `b` is shorter than 8
bytes**; check the length yourself when a short input must be an error.

## What the SDK does not provide

There is no way to learn the caller, the transaction id, the contract's own
id, to call another contract, to move ZEC, or to emit events. Contracts that
need authorization must encode it in their own input and state, and must
treat all input as untrusted. See [contract-model.md](contract-model.md).

## Complete minimal counter

This is `contracts/counter/src/lib.rs` as shipped.

```rust
#![no_std]
#![no_main]

#[global_allocator]
static ALLOC: wee_alloc::WeeAlloc = wee_alloc::WeeAlloc::INIT;
extern crate alloc;

use zalkanes_sdk as sdk;

const KEY: &[u8] = b"counter";

#[no_mangle]
pub extern "C" fn dispatch(opcode: i32, _input_len: i32) -> i32 {
    match opcode as u16 {
        // increment()
        0x0001 => {
            let current = sdk::get(KEY).map(|b| sdk::bytes_to_u64(&b)).unwrap_or(0);
            let next = current.wrapping_add(1);
            sdk::set(KEY, &sdk::u64_to_bytes(next));
            0
        }
        // get() -> big-endian u64
        0x0002 => {
            let val = sdk::get(KEY).map(|b| sdk::bytes_to_u64(&b)).unwrap_or(0);
            let bytes = sdk::u64_to_bytes(val);
            if sdk::write_output(&bytes) {
                0
            } else {
                -1
            }
        }
        // initialize(initial_value: u64)
        0x0003 => {
            let input = sdk::read_input();
            if input.len() < 8 {
                return -1;
            }
            let val = sdk::bytes_to_u64(&input[..8]);
            sdk::set(KEY, &sdk::u64_to_bytes(val));
            0
        }
        _ => -1,
    }
}

// Required for no_std panic handler
#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}
```

Its `Cargo.toml`:

```toml
[package]
name = "counter"
version = "0.1.0"
edition = "2021"
publish = false

[workspace]

[lib]
crate-type = ["cdylib"]

[dependencies]
zalkanes-sdk = { path = "../../crates/zalkanes-sdk" }
wee_alloc = "=0.4.5"

[profile.release]
opt-level = "s"
lto = true
panic = "abort"
strip = true
```

Note the `[workspace]` table: it makes the contract its own workspace so that
`cargo test --workspace` at the repository root stays host-only.

## A simple key/value contract

`contracts/key-value/src/lib.rs`, unchanged. Opcode 1 takes
`key_len (u16 BE) || key || value`; opcodes 2 and 3 take the bare key.

```rust
#![no_std]
#![no_main]

#[global_allocator]
static ALLOC: wee_alloc::WeeAlloc = wee_alloc::WeeAlloc::INIT;
extern crate alloc;
use zalkanes_sdk as sdk;

#[no_mangle]
pub extern "C" fn dispatch(opcode: i32, _input_len: i32) -> i32 {
    let input = sdk::read_input();
    match opcode as u16 {
        // set(key_len: u16 BE, key: bytes, value: bytes)
        0x0001 => {
            if input.len() < 2 {
                return -1;
            }
            let key_len = u16::from_be_bytes([input[0], input[1]]) as usize;
            if input.len() < 2 + key_len {
                return -1;
            }
            let key = &input[2..2 + key_len];
            let value = &input[2 + key_len..];
            sdk::set(key, value);
            0
        }
        // get(key: bytes) -> value
        0x0002 => match sdk::get(&input) {
            Some(v) => {
                sdk::write_output(&v);
                0
            }
            None => -1,
        },
        // delete(key: bytes)
        0x0003 => {
            sdk::delete(&input);
            0
        }
        _ => -1,
    }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}
```

A `get` of a missing key returns `-1`, so it is recorded as a failed
execution; that is deliberate, and [contracts/key-value/README.md](../../contracts/key-value/README.md)
shows what it looks like on chain.

## Testing contracts in-process

`crates/zalkanes-testkit` drives the real block processor against an
in-memory store without Zebra. `crates/zalkanes-testkit/tests/e2e.rs` is the
template: `TestChain::new()`, `deploy(&wasm)`, `call(id, opcode, &input)`,
`view(id, opcode, &input)`, `state_root()`. Copy the pattern into your own
integration test and point `include_bytes!` at your built `.wasm`.
