# counter

The reference contract: one `u64` in storage, three opcodes. This is the
contract the quickstart deploys, the testkit's fixture
(`crates/zalkanes-testkit/fixtures/counter.wasm`), and the module used in
the recorded testnet evidence. Source: `src/lib.rs` (about 50 lines).

## What it demonstrates

- The `dispatch(opcode, input_len) -> i32` entrypoint and opcode matching.
- Reading and writing one storage key (`b"counter"`).
- Returning data with `write_output`, encoded as a big-endian `u64`.
- Reading call input (`initialize`).
- Returning `-1` to reject an unknown opcode or a short input.

## Opcodes

| opcode | name | input | output | effect |
|---|---|---|---|---|
| `1` | `increment` | none | none | `counter += 1` (wrapping) |
| `2` | `get` | none | `u64` big-endian (8 bytes) | none |
| `3` | `initialize` | `u64` big-endian (8 bytes) | none | `counter = input` |

Any other opcode returns `-1` (recorded as `dispatch returned error code -1`).
`initialize` with fewer than 8 input bytes also returns `-1`.

## Build

```bash
zalkanes contract build --manifest-path ./contracts/counter
```

Output: `./contracts/counter/target/wasm32-unknown-unknown/release/counter.wasm`
(2,513 bytes; SHA-256 `fa8289fbc0fdb132e57f939033a9971b0ee2990ec49db247aad3f682aa804cc7`
with the pinned toolchain).

## Deploy (regtest)

```bash
zalkanes contract deploy ./contracts/counter/target/wasm32-unknown-unknown/release/counter.wasm --funding transparent --yes --wait
```

Two carrier chunks; PREPARE fee 15,000 zat, DEPLOY fee 95,000 zat. Copy the
`ContractId:` line into `CONTRACT_ID`.

## Call

```bash
zalkanes contract call "$CONTRACT_ID" 1 "" --funding transparent --yes --wait
zalkanes contract call "$CONTRACT_ID" 3 000000000000002a --funding transparent --yes --wait
```

The first increments; the second sets the counter to 42 (`0x2a`). Each is a
Zcash transaction. `--wait` prints the execution record; `increment` uses
3,936 fuel.

## View

```bash
zalkanes contract view "$CONTRACT_ID" 2 "" --rpc-url http://127.0.0.1:3030
```

```json
{"jsonrpc":"2.0","id":1,"result":{"success":true,"output_hex":"0000000000000002","fuel_used":3936,"indexed_height":444,"state_root":"fb6b7f36…","error":null}}
```

Decode `output_hex` as a big-endian `u64`: after two increments it is `2`.
An unknown opcode viewed:

```json
{"result":{"success":false,"output_hex":"","fuel_used":316,"error":"dispatch returned error code -1",…}}
```

## Notes

- Anyone can call any opcode, including `initialize`; v0 contracts have no
  caller identity. That is fine for a demonstration and is the reason the
  token example is not a real token.
- `docs/developers/sdk.md` reproduces the full source with commentary.
