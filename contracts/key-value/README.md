# key-value

A byte-keyed, byte-valued store with `set`, `get`, and `delete`. Source:
`src/lib.rs`. Supported v0 developer example.

## What it demonstrates

- Variable-length keys and values, length-prefixed in the input.
- `storage_delete`.
- A read of a missing key that deliberately fails the call (`-1`), showing
  what a failed execution looks like on chain and in a view.

## Opcodes

| opcode | name | input | output | effect |
|---|---|---|---|---|
| `1` | `set` | `key_len (u16 BE) ‖ key ‖ value` | none | stores `value` under `key` |
| `2` | `get` | `key` | `value` | none; returns `-1` if absent |
| `3` | `delete` | `key` | none | removes `key` (succeeds even if absent) |

`set` returns `-1` if the input is shorter than `2 + key_len`. Keys are at
most 256 bytes and values at most 64 KiB, but through the CLI the whole input
must fit in 38 bytes (`2 + key_len + value_len ≤ 38`); see
`docs/developers/calling-contracts.md`.

## Build

```bash
zalkanes contract build --manifest-path ./contracts/key-value
```

Output: `./contracts/key-value/target/wasm32-unknown-unknown/release/key_value.wasm`
(1,976 bytes).

## Deploy (regtest)

```bash
zalkanes contract deploy ./contracts/key-value/target/wasm32-unknown-unknown/release/key_value.wasm --funding transparent --yes --wait
```

## Call

Store `"foo" → "bar"`: `key_len = 0x0003`, key `666f6f`, value `626172`.

```bash
zalkanes contract call "$KV" 1 0003666f6f626172 --funding transparent --yes --wait
```

Delete `"foo"`:

```bash
zalkanes contract call "$KV" 3 666f6f --funding transparent --yes --wait
```

A `get` sent as a CALL is pointless (its output is only visible in the
execution record) but it is a clean example of a failing call. Reading the
missing key `"nope"`:

```bash
zalkanes contract call "$KV" 2 6e6f7065 --funding transparent --yes --wait
```

```text
CALL mined at height 888: cd377e47…
execution: {…,"success":false,"error":"dispatch returned error code -1","fuel_used":7415,…}
```

The transaction is in block 888, the fee was paid, and no state changed.

## View

```bash
zalkanes contract view "$KV" 2 666f6f --rpc-url http://127.0.0.1:3030
```

```json
{"jsonrpc":"2.0","id":1,"result":{"success":true,"output_hex":"626172","fuel_used":7415,"indexed_height":777,"state_root":"3a67a5cc…","error":null}}
```

`626172` is `"bar"`. After the delete, the same view fails with
`dispatch returned error code -1`.

## Notes

- The `get` opcode fails on a missing key so that "absent" and "empty value"
  are distinguishable. Returning an empty output with `0` would be the other
  reasonable design.
- There is no namespacing or access control: every caller shares one key
  space and anyone can overwrite or delete any key.
