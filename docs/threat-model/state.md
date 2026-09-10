# Threat Model: State

## Attacker goal

Corrupt the state database, cause state-root divergence between honest nodes,
or inject false state that survives a restart or reorg.

## Attacks and mitigations

### State divergence via nondeterministic iteration

**Attack:** Rely on hashmap iteration order or RocksDB scan order being
implementation-defined, causing nodes on different hardware/OS to compute
different state roots.

**Mitigation:** State root is computed over leaves sorted lexicographically by
their hash (see ADR 0006). RocksDB is accessed with explicit key-sorted prefix
scans. No hashmap iteration touches consensus state.

### Partial write on crash

**Attack:** Kill the process mid-block-commit, leaving partially applied state.

**Mitigation:** Every block commit is an atomic RocksDB WriteBatch. Either all
writes are applied or none are. The indexer tip pointer is updated in the same
batch. After restart, the node resumes from the last fully committed height.

### Replay attack (index same block twice)

**Attack:** Trigger a bug that causes the indexer to process the same block
height twice.

**Mitigation:** The indexer checks `current_tip + 1 == incoming_height` before
processing. Re-processing an already-committed height is detected and skipped
(idempotent replay). ContractId uniqueness is enforced by the state engine.

### Fake state root injection via RPC

**Attack:** A malicious local process writes to the Zebra RPC socket, supplying
a forged block with altered transactions.

**Mitigation:** Zalkanes parses the full raw block bytes from Zebra and
verifies that the protocol data matches the committed code_hash before any
state mutation. Zebra itself validates all Zcash consensus rules. Zalkanes
trusts Zebra's consensus validation as its security anchor.

### Storage key/value overflow

**Attack:** Craft a contract that writes a storage key or value exceeding
declared limits, hoping the state engine stores it anyway.

**Mitigation:** The host ABI enforces MAX_STORAGE_KEY_BYTES and
MAX_STORAGE_VALUE_BYTES before any write reaches the state engine.

### State root mismatch after restart

**Attack:** Rely on in-memory caches or temporary state not being persisted,
causing the root to differ after a restart.

**Mitigation:** The state root is recomputed deterministically from persisted
RocksDB data on startup verification. In-memory caches are never authoritative.

## Test coverage

Covered by the storage test suite (spec §36), reorg test suite (spec §22-23),
crash consistency tests (spec §24), and determinism tests (spec §33).
