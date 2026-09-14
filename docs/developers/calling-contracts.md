# Calling contracts: CALL and VIEW

Two very different operations share the same contract code path.

| | CALL | VIEW |
|---|---|---|
| what it is | a real Zcash transaction carrying a ZALK message | a JSON-RPC request to a Zalkanes node |
| costs | the Zcash transaction fee (ZIP-317; 15,000 zat for an empty-input call on regtest) | nothing |
| changes state | yes, if the contract succeeds; every node applies the same writes | never; writes are discarded |
| can fail | yes: trap, non-zero return, fuel exhaustion | yes, same reasons, reported in the response |
| effect of failure | the Zcash transaction is still valid and mined; the fee is spent; no state change; execution record says `success:false` | none |
| when it executes | when the block containing the transaction is indexed, in block order | immediately, against the node's current indexed state |
| who sees it | everyone, forever, on chain | only you and the node you asked |

## CALL

```bash
zalkanes contract call <contract-id> <opcode> [input-hex] --funding transparent --yes --wait
```

- `<contract-id>`: 64 hex characters.
- `<opcode>`: decimal `u16` (`1`, not `0x0001`).
- `[input-hex]`: hex-encoded input bytes; omit or pass `""` for none.
- `--funding`, `--yes`, `--dry-run`, `--wait`, `--confirm-mainnet`: as for
  deploy ([deploying.md](deploying.md)).

The CLI encodes the message, funds a transaction that carries it in an
OP_RETURN, signs, broadcasts, and (on regtest) mines it:

```text
ZALK payload: 5a414c4b00022d7e818e…00010000
broadcast accepted: 4503507f…
CALL mined at height 333: 4503507f…
```

The payload is `ZALK || version 0 || type 2 || contract_id || opcode (u16 BE)
|| input_length (u16 BE) || input`. It is identical whichever pool funded
the transaction.

### Input size

The CLI builds **inline** CALL messages, which fit in one OP_RETURN, so the
input is limited to `max_call_inline_bytes` (38 bytes; see
[protocol-limits.md](protocol-limits.md)). Larger inputs are refused
locally:

```text
Error: input is 48 bytes but an inline CALL carries at most 38 bytes; larger inputs need a CALL_CARRIER transaction, which this CLI does not build yet
```

The protocol defines a carrier-delivered CALL for inputs up to 64 KiB, and
nodes parse it, but the CLI does not construct it in this release. Design
contract inputs to fit 38 bytes, or split work across calls.

### What `--wait` shows

`--wait` polls `zalkanes_getExecution` for the call's txid and prints the
execution record. Its fields (byte arrays are shown as JSON arrays):

| field | meaning |
|---|---|
| `success` | `true` if `dispatch` returned `0` |
| `fuel_used` | fuel consumed, also on failure |
| `return_data` | output bytes on success, empty on failure |
| `error` | `null`, or the failure reason (`dispatch returned error code -1`, `fuel exhausted`, a trap message) |
| `state_root_before`, `state_root_after` | roots around this execution |
| `block_height`, `block_hash`, `txid`, `contract_id`, `opcode` | where and what |

A failed call on chain, from the key-value example (`get` of a missing key):

```text
CALL mined at height 888: cd377e47…
success: False  error: dispatch returned error code -1  fuel: 7415
```

The transaction is in block 888; the state root did not change.

### Ordering

Calls execute in the order their transactions appear in the block, blocks in
chain order. Two calls in one block see each other's effects in that order.
A Zcash reorg rolls executions back and replays them on the new branch; the
CLI's `--wait` and the node's RPC always reflect the canonical chain.

## VIEW

```bash
zalkanes contract view <contract-id> <opcode> [input-hex] --rpc-url http://127.0.0.1:3030
```

`--rpc-url` defaults to `$ZALKANES_URL`, then `http://127.0.0.1:3030`. The
command is a thin wrapper over JSON-RPC:

```bash
curl -s -X POST http://127.0.0.1:3030 -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"zalkanes_view","params":["<contract-id>", 2, ""]}'
```

Response:

```json
{"jsonrpc":"2.0","id":1,"result":{"success":true,"output_hex":"0000000000000002","fuel_used":3936,"indexed_height":444,"state_root":"fb6b7f36…","error":null}}
```

A view runs the same module with the full per-call fuel limit against a
read-only snapshot at `indexed_height`, so `state_root` tells you exactly
which state the answer describes. A view has no input size limit beyond the
protocol's 64 KiB, so it can exercise opcodes the CLI cannot yet call.

A failing view:

```json
{"result":{"success":false,"output_hex":"","fuel_used":316,"indexed_height":444,"state_root":"…","error":"dispatch returned error code -1"}}
```

An unknown contract id returns `"error":"invalid contract id"` (malformed) or
`"contract not found"` (well-formed but not deployed).

## Other read-only RPC methods

| method | params | returns |
|---|---|---|
| `zalkanes_getInfo` | `[]` | protocol version, manifest hash, network, indexed height and hash, chain tip, state root, `syncing` |
| `zalkanes_getStateRoot` | `[]` | the current state root |
| `zalkanes_getContract` | `[id]` | `contract_id`, `code_hash`, `code_size`, or `null` |
| `zalkanes_getCode` | `[id]` | the WASM as hex |
| `zalkanes_view` | `[id, opcode, input_hex]` | as above |
| `zalkanes_getExecution` | `[txid]` | one execution record, or `null` |
| `zalkanes_getBlockExecutions` | `[height]` | all execution records in that block |

`txid` is in display order (as Zebra and explorers print it). The JSON-RPC
server has no authentication and no rate limiting; keep it private or behind
a proxy.
