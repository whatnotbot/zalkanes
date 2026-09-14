# token

A `u128` balance ledger with `initialize`, `balance_of`, `transfer`, `mint`,
`burn`, and `total_supply`. Source: `src/lib.rs`.

**Educational example only. This is not a token implementation you can use.**
Protocol v0 gives a contract no authenticated caller identity, so this
contract has no access control at all: `transfer` moves funds from whatever
`from` address the input names, `mint` and `burn` are callable by anyone, and
the stored `owner` is never checked. It exists to show a multi-key storage
layout and `u128` encoding, nothing more. It is not a template for an asset,
and this repository does not build asset, exchange, or bridge applications.

## What it demonstrates

- Composite storage keys (`b"balance/" ‖ addr`) alongside singleton keys
  (`b"supply"`, `b"owner"`).
- Fixed-width `u128` big-endian arithmetic with overflow checks.
- Inputs larger than the CLI can send inline.

## Opcodes

| opcode | name | input | output |
|---|---|---|---|
| `1` | `initialize` | `supply (u128 BE, 16) ‖ owner (32)` | none |
| `2` | `balance_of` | `addr (32)` | `u128 BE` |
| `3` | `transfer` | `from (32) ‖ to (32) ‖ amount (u128 BE, 16)` | none; `-1` if `from` lacks funds |
| `4` | `mint` | `to (32) ‖ amount (16)` | none; `-1` on supply overflow |
| `5` | `burn` | `from (32) ‖ amount (16)` | none; `-1` if `from` lacks funds |
| `6` | `total_supply` | none | `u128 BE` |

"Addresses" are arbitrary 32-byte labels chosen by the caller; nothing ties
them to keys or Zcash addresses.

## Build and deploy

```bash
zalkanes contract build --manifest-path ./contracts/token
zalkanes contract deploy ./contracts/token/target/wasm32-unknown-unknown/release/token.wasm --funding transparent --yes --wait
```

6,723 bytes, 5 carrier chunks, DEPLOY fee 255,000 zat on regtest (verified).

## What the CLI can and cannot exercise

The CLI sends inline CALL messages with at most 38 bytes of input.
`initialize` (48 bytes), `transfer` (80), `mint` (48), and `burn` (48) all
exceed that and are refused before broadcast:

```text
Error: input is 48 bytes but an inline CALL carries at most 38 bytes; larger inputs need a CALL_CARRIER transaction, which this CLI does not build yet
```

`balance_of` (32 bytes) and `total_supply` (0 bytes) fit, and views have no
such limit, so the read side works today:

```bash
zalkanes contract view "$TK" 6 "" --rpc-url http://127.0.0.1:3030
zalkanes contract view "$TK" 2 1111111111111111111111111111111111111111111111111111111111111111 --rpc-url http://127.0.0.1:3030
```

Both return `00000000000000000000000000000000` on a fresh deploy. A view can
also exercise the large-input opcodes without a transaction, for example a
`transfer` that must fail:

```bash
FROM=$(printf '22%.0s' {1..32}); TO=$(printf '11%.0s' {1..32})
zalkanes contract view "$TK" 3 "${FROM}${TO}00000000000000000000000000000fff" --rpc-url http://127.0.0.1:3030
```

returns `"success":false,"error":"dispatch returned error code -1"` (the
three fields are concatenated into one hex argument).

To drive the state-changing opcodes end to end, use the in-process testkit
(`crates/zalkanes-testkit`), which feeds inputs of any length to the real
block processor. See `docs/developers/sdk.md`.
