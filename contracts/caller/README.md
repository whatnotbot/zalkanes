# caller

**Not deployable on protocol v0. Unsupported. Kept for illustration only.**

This crate imports a host function, `env::contract_call`, that the v0 host
ABI does not provide. Consensus validation rejects any module with an unknown
import, so a deploy fails before spending anything:

```text
Error: WASM validation failed: unresolvable import env::contract_call (host ABI provides only env::{storage_get, storage_set, storage_delete, output_write, context_block_height, input_read})
```

(That is the exact output of `zalkanes contract deploy` on regtest for this
module.) The same rejection happens on every node if such a transaction were
somehow broadcast, so a caller contract can never enter consensus state on
v0. No test in this repository deploys it.

## Why it exists

It shows two shapes that contract authors ask about:

- what a nested call into another contract would look like as an `extern "C"`
  import (opcodes `1` and `2`, targeting the counter's `increment` and `get`),
- how a contract deliberately traps to guarantee rollback of an earlier
  storage write (opcode `3` writes `b"before_trap"` and then executes
  `unreachable`).

The trap-rollback behaviour is real on v0 and applies to any contract: a
trap discards every write of that call. See
`docs/developers/contract-model.md`. The nested-call import is **not** part
of v0, and this repository does not add it; there is no cross-contract call
in protocol v0 (`audit/README.md`, "What an auditor should know up front").

## Opcodes (as written, not executable on v0)

| opcode | name | input |
|---|---|---|
| `1` | `call_counter` | target `ContractId` (32) |
| `2` | `nested_success` | target `ContractId` (32) |
| `3` | `nested_failure` | none |

## Build

It compiles, which is useful only to confirm the toolchain and to reproduce
the rejection above:

```bash
zalkanes contract build --manifest-path ./contracts/caller
```

Use `counter` and `key-value` as your starting points instead.
