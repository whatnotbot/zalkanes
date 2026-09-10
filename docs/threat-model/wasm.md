# Threat Model: WASM Execution

## Attacker goal

Cause CPU exhaustion, memory exhaustion, stack overflow, nondeterministic
execution, partial state mutation on trap, or cross-contract information leaks.

## Attack surface

- The WASM module bytes submitted in a DEPLOY transaction.
- The input bytes submitted in a CALL transaction.
- The host ABI exposed by the Zalkanes runtime.
- Contract-to-contract call parameters.

## Attacks and mitigations

### CPU exhaustion (infinite loop)

**Attack:** Deploy a WASM module containing an infinite loop.

**Mitigation:** Fuel metering via Wasmi. Every instruction deducts fuel.
`MAX_FUEL_PER_CALL` is reached and execution traps with `FuelExhausted`. The
trap is deterministic and produces no state mutation.

### Memory exhaustion

**Attack:** Call `memory.grow` in a tight loop to exhaust host memory.

**Mitigation:** `MAX_LINEAR_MEMORY_PAGES = 256` (16 MiB). Grow attempts beyond
this return -1 (Wasm semantics). The host allocates the full linear memory
upfront bounded by the page limit.

### Stack overflow (recursive calls)

**Attack:** Implement deeply recursive WASM functions to overflow the host
call stack.

**Mitigation:** Wasmi uses an explicit call stack with a configurable depth
limit, not the host thread stack. `MAX_CALL_DEPTH` bounds this.

### Floating-point nondeterminism

**Attack:** Use `f32`/`f64` instructions whose behavior differs across CPU
architectures (NaN canonicalization, denormal handling).

**Mitigation:** Float instructions are forbidden at module validation time.
Any module containing float instructions is rejected before execution.

### Partial state on trap

**Attack:** Write to storage, then intentionally trap (e.g., `unreachable`),
hoping partial writes persist.

**Mitigation:** All storage writes during a call go to an overlay. The overlay
is committed atomically only on success. Any trap → overlay discarded.

### Huge storage write

**Attack:** Call `storage_set` with a key or value exceeding the maximum size.

**Mitigation:** Host function checks `key_len <= MAX_STORAGE_KEY_BYTES` and
`val_len <= MAX_STORAGE_VALUE_BYTES` before any allocation. Returns error code
-1; does not panic.

### Huge return value

**Attack:** Call `output_write` with `len` = u32::MAX.

**Mitigation:** `output_write` checks `len <= MAX_RETURN_DATA_BYTES` and returns
-1 if exceeded. The WASM memory bounds check also applies.

### OOB memory access via host function

**Attack:** Pass a pointer + length to a host function that references memory
outside the WASM linear memory.

**Mitigation:** Every host function that accepts `(ptr, len)` validates that
`ptr + len <= memory.size()` before any read/write. Out-of-bounds returns -1.

### Cross-contract information leak

**Attack:** Call another contract, then read its storage directly.

**Mitigation:** Storage is namespaced by ContractId. `storage_get` / `storage_set`
always operate under the current contract's namespace. A contract cannot
directly access another contract's storage namespace.

### Reentrancy / recursive call abuse

**Attack:** Contract A calls B, B calls A back to exploit partial state.

**Mitigation:** Nested calls operate on overlays. Parent overlay is not visible
to child. Child overlay is committed to parent overlay on success, or discarded
on failure. `MAX_CALL_DEPTH` prevents unbounded recursion.

### Unknown import injection

**Attack:** Submit a WASM module that imports a function not in the v0 host ABI,
hoping the runtime provides it anyway.

**Mitigation:** Module validation rejects any module with imports not in the
known host ABI list. Deployment fails with no state mutation.

### Thread or SIMD instructions

**Attack:** Use `threads` or `simd` proposals to introduce nondeterminism.

**Mitigation:** These WASM proposals are disabled in the Wasmi v0 configuration.
Module validation rejects modules containing these instructions.

## Test coverage

All scenarios are covered by the WASM adversarial test suite (spec §35),
the fuzz target `fuzz/wasm-validator/`, and the fuel test vectors
`test-vectors/wasm/fuel-v0.json`.
