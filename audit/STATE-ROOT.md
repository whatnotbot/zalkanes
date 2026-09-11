# State root — frozen specification and evidence

## Algorithm (protocol v0, FROZEN)

The state root is a **flat sorted BLAKE2b-256 hash**, not a Merkle tree
(ADR-0006 records the rationale; incremental proofs are deferred to a future
protocol version).

```
contract_leaf = BLAKE2b-256(person="ZalkStateLeaf0  ",
                            contract_id(32) || code_hash(32) || code_len(u32 BE) || wasm)

storage_leaf  = BLAKE2b-256(person="ZalkStateLeaf0  ",
                            contract_id(32) || key_len(u16 BE) || key
                                            || val_len(u32 BE) || value)

state_root    = BLAKE2b-256(person="ZalkStateRoot0  ",
                            concat(sort_lexicographic(all_leaves)))
```

Personalization strings are exactly 16 bytes (trailing spaces are
significant). All integers are big-endian.

**What the root commits to:** deployed contract code (id, code hash, length,
bytes) and storage entries (contract id, key, value). Nothing else — not
heights, not block hashes, not deployment metadata. `docs/adr/0006-state-root.md`
previously described a larger commitment; the implementation and this
document are authoritative.

**Empty state root:** `b120099c167d...` — this value is reproduced by the
Python reference implementation AND matches the live public-testnet chain's
recorded empty root.

## Implementation

- `crates/zalkanes-state/src/lib.rs` — `contract_leaf`, `storage_leaf`,
  `root_from_leaves`, `collect_leaves`, `projected_root`.
- `compute_root()` over the store is always authoritative; the persisted
  `m:r` value is written in the same atomic batch as the commit.

## Independent verifier

`tools/zalkanes-reference/reference.py` is a standalone Python implementation
of the hashing primitives (contract id, both leaf kinds, the root, carrier
reconstruction, OP_RETURN decoding), written against the specification. It
generates `test-vectors/state-root/vectors.json` (100 vectors), which the
Rust suite consumes in `crates/zalkanes-state/tests/state_root_vectors.rs`.

## Committed vectors

| file | count | consumed by |
|---|---|---|
| `test-vectors/state-root/vectors.json` | 100 | `zalkanes-state/tests/state_root_vectors.rs` |
| `test-vectors/execution/v1.json` | 101 | `zalkanes-testkit/tests/execution_vectors.rs` |
| `test-vectors/state-roots/v0.json` | 2 | `zalkanes-state/tests/state_root_v0_vectors.rs` |
| `test-vectors/protocol/contract-id-v0.json` | 3 | `zalkanes-core/tests/contract_id_vectors.rs` |

The 101 execution vectors are produced by driving the **real production block
processor** and record, per block: height, description, resulting state root,
and every execution's success flag, fuel used, and error class. Scenario
coverage: deploy, 70 calls, initialize-with-input, unknown opcode, call to a
missing contract, malformed/foreign OP_RETURN payloads (parser skip and error
paths), fuel exhaustion, storage write, same-call write-then-delete,
trap-after-write (writes discarded), a second instance of identical code,
multi-transaction blocks (locking in v0 intra-block visibility semantics),
no-op blocks, and rollback to a recorded height reproducing that height's
root. Regenerate with `ZALKANES_GENERATE_VECTORS=1`.

Nested cross-contract calls are **not** covered because they do not exist in
protocol v0 (there is no `contract_call` host function).

## Determinism evidence

| configuration | status |
|---|---|
| aarch64-apple-darwin, debug | green (native host) |
| aarch64-apple-darwin, release | green (native host) |
| x86_64-apple-darwin, debug | green (**Rosetta 2 emulation**) |
| x86_64-apple-darwin, release | green (**Rosetta 2 emulation**) |
| x86_64-unknown-linux-gnu, debug + release | green (native, GitHub Actions `CI`) |
| aarch64-unknown-linux-gnu, debug + release | green (native, GitHub Actions `arm64-determinism`) |

All configurations verify the committed execution vectors, so success/failure
classification, fuel, return bytes, and state roots are byte-identical across
both architectures and both profiles.

## Two-node equality

- **Live:** an independent local indexer with its own RocksDB resynced the
  entire post-activation public-testnet chain and reproduced the Railway
  indexer's root byte-for-byte at height 4,339,534
  (`28256d5f65020dddcdd5befc70f4d80f25de605232765980c3f87c1136e86f95`).
- **At scale:** `crates/zalkanes-testkit/tests/two_node_stress.rs` replays 100
  deployments, 10,000 successful calls and 1,000 intentional failures through
  the real block processor in two isolated state databases, asserting root
  equality at every block.
