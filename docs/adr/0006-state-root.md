# ADR 0006 — State Root

**Status:** Accepted  
**Date:** 2025-09-10

## Context

Every processed block must produce a `zalkanes_state_root` that all honest
nodes agree on. The state root must commit to all consensus-affecting state.

## Decision

### What is committed

- Contract code identity: `(contract_id, code_hash)`
- Contract storage: all `(contract_id, key, value)` triples
- Contract metadata: `(contract_id, deployment_height, deployment_txid, protocol_version)`
- Protocol metadata: indexed height, Zcash block hash

### What is NOT committed

- Logs, metrics, RPC cache, local timestamps, peer information, non-consensus indexes.

### Algorithm

```
leaf(contract_id, key, value) =
    BLAKE2b-256(
        personalization = "ZalkStateLeaf0  ",   // 16 bytes
        input = contract_id (32) || key_len (2, BE) || key || value_len (4, BE) || value
    )

state_root =
    BLAKE2b-256(
        personalization = "ZalkStateRoot0  ",   // 16 bytes
        input = sorted_leaves_concatenated
    )
```

Sorting: lexicographic on the 32-byte leaf hash (big-endian byte comparison).

This is a flat sorted hash — not a Merkle tree. A Merkle tree may be adopted in
a future protocol version for incremental proofs. The flat design is simpler to
implement correctly for v0.

### Rationale

- BLAKE2b matches Zcash ecosystem conventions.
- Deterministic sorting eliminates map-iteration-order nondeterminism.
- Personalization strings include a version digit to allow future algorithm changes.
- Flat sorted hash is simple, fast, and easy to test.

## Test vectors

File: `test-vectors/state-roots/v0.json`

Each vector:
```json
{
  "protocol": 0,
  "height": 5,
  "zcash_block_hash": "...",
  "previous_root": "...",
  "state_entries": [
    { "contract_id": "...", "key_hex": "...", "value_hex": "..." }
  ],
  "expected_root": "..."
}
```

Vectors are immutable after v0 activation. CI fails on regression.

## Consequences

- State root is 32 bytes (BLAKE2b-256 output).
- All state must be iterable in deterministic order at commit time.
- RocksDB iteration order must be canonicalized (key-sorted prefix scan).
- Changing this algorithm requires a new protocol version.
