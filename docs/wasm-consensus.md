# WASM Consensus Profile — v0

> **Consensus-critical. Do not change without opening a new protocol version ADR and regenerating `test-vectors/wasm/fuel-v0.json`.**

---

## 1. Runtime

| Field               | Value                              |
|---------------------|------------------------------------|
| Crate               | `wasmi`                            |
| Pinned version      | `=2.0.0`                           |
| Repository          | https://github.com/wasmi-labs/wasmi |
| Cargo.lock committed | yes                               |
| Feature flags       | `no-std`                           |

Upgrading Wasmi is a **protocol upgrade** unless fuel equivalence is formally
proven for all existing test vectors.

---

## 2. WASM feature set

| Feature          | Status in v0 |
|------------------|--------------|
| MVP instructions | allowed      |
| Floats (f32/f64) | **forbidden** — modules containing float instructions are rejected at validation |
| Sign-extension   | allowed      |
| Saturating trunc | forbidden    |
| Bulk memory      | forbidden    |
| Threads          | forbidden    |
| SIMD             | forbidden    |
| Relaxed SIMD     | forbidden    |
| memory64         | forbidden    |
| Multi-value      | allowed      |
| Reference types  | forbidden    |
| Component model  | forbidden    |

Modules importing unknown functions: **rejected**.
Modules with start functions: **rejected**.

---

## 3. Fuel configuration

Fuel is consensus-critical.  Given identical WASM, input, and prior state, all
nodes MUST consume identical fuel and trap at the same point.

Fuel is deducted per Wasmi instruction per the Wasmi 2.0.0 metering table.
No custom fuel schedule overrides are applied in v0.

Per-call limit: `MAX_FUEL_PER_CALL` (see `zalkanes-core::consensus`).

Fuel is NOT refunded to the caller on trap.
Unused fuel from a sub-call is NOT returned to the parent in v0.
(This is consensus-explicit; see §37 of spec.)

---

## 4. Memory limits

| Limit                    | Value                 |
|--------------------------|-----------------------|
| MAX_LINEAR_MEMORY_PAGES  | 256 (= 16 MiB)        |
| Initial pages allowed    | 0..=256               |
| `memory.grow` allowed    | yes, up to page limit |
| Multi-memory             | forbidden             |

Memory grow beyond the limit returns -1 (Wasm semantics); does not trap.

---

## 5. Module validation

Validation is performed before execution and before any state is committed.

Steps:
1. Byte-length check: `len(wasm) <= MAX_CODE_BYTES`.
2. Wasmi module parsing.
3. Feature-set check (floats, threads, SIMD, etc. — see §2).
4. Import check: every import MUST resolve to a known host function.
5. Start function: REJECT if present.

A module that fails validation is stored as invalid; the deployment message is
treated as a no-op (no state mutation, no ContractId registered).

---

## 6. Host ABI (v0)

```
storage_get(key_ptr: i32, key_len: i32, val_ptr: i32) -> i32
    Returns value length, or -1 if not found.

storage_set(key_ptr: i32, key_len: i32, val_ptr: i32, val_len: i32) -> i32
    Returns 0 on success, -1 on limit exceeded.

storage_delete(key_ptr: i32, key_len: i32) -> i32
    Returns 0 on success.

context_contract_id(out_ptr: i32) -> ()
    Writes 32 bytes of ContractId into WASM memory.

context_caller(out_ptr: i32) -> i32
    Writes 32 bytes of caller ContractId, or zeros if top-level. Returns 0.

context_txid(out_ptr: i32) -> ()
    Writes 32 bytes of ZIP-244 txid.

context_block_height(out_ptr: i32) -> i32
    Writes u32 block height big-endian. Returns 0.

input_read(out_ptr: i32, offset: i32, len: i32) -> i32
    Copies input bytes. Returns bytes written.

output_write(ptr: i32, len: i32) -> i32
    Sets call return data. Returns 0 on success, -1 if len > MAX_RETURN_DATA_BYTES.

contract_call(id_ptr: i32, opcode: i32, in_ptr: i32, in_len: i32,
              out_ptr: i32, out_max: i32) -> i32
    Synchronous sub-call. Returns output length, or negative error code.
```

All host functions that accept pointer+length pairs MUST check that the
referenced memory range is within the module's linear memory.
Out-of-bounds access returns an error code, not a host panic.

---

## 7. Trap semantics

| Condition                         | Result                            |
|-----------------------------------|-----------------------------------|
| WASM trap (unreachable, OOB, etc.)| execution halted, overlay discarded |
| Fuel exhaustion                   | execution halted, overlay discarded |
| Invalid return encoding           | overlay discarded                 |
| Host function error (bounds etc.) | error code returned to WASM       |

Partial storage writes from a trapped execution are NEVER committed.

---

## 8. View calls

View calls run identical WASM with a fresh read-only overlay.
All storage writes during a view operate against the overlay and are discarded.
The view result is the return data written via `output_write`.

---

## 9. Determinism requirements

The execution profile MUST be identical across:
- x86_64-unknown-linux-gnu
- aarch64-unknown-linux-gnu

If any fuel difference is observed between architectures with the same Wasmi
version, this is a **stop condition** (see AGENTS.md) and requires an ADR.

---

## 10. Fuel test vectors

File: `test-vectors/wasm/fuel-v0.json`

Each vector:
```json
{
  "wasm_hash": "<sha256 hex>",
  "input_hex": "...",
  "starting_state_root": "...",
  "fuel_limit": 10000000,
  "expected_fuel_consumed": 12345,
  "expected_result": "success|trap|fuel_exhausted",
  "expected_state_root": "...",
  "expected_output_hex": "..."
}
```

These vectors are immutable after v0 activation.  CI fails if code changes them.
