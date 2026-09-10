# Threat Model: Chain Reorg

## Attacker goal

Cause honest Zalkanes nodes to diverge on state after a reorg, corrupt state
during rollback, or prevent correct replay of the new canonical chain.

## Attacks and mitigations

### Partial rollback on crash

**Attack:** Kill the node mid-rollback, leaving state that is neither the pre-reorg
state nor the post-reorg state.

**Mitigation:** Rollback steps are atomic RocksDB WriteBatches. The rollback
cursor is persisted atomically. After restart, the node detects the incomplete
rollback and resumes from the cursor.

### Reorg depth exceeds rollback data

**Attack:** Trigger a reorg deeper than the stored rollback history.

**Mitigation:** In v0, rollback history is stored for all indexed heights (full
historical state). A reorg to genesis is theoretically supported. Production
deployments may configure a minimum stored depth; reorgs exceeding it require
a full re-index.

### Different canonical chain after reorg

**Attack:** Two nodes disagree on which chain is canonical after a reorg because
they received blocks from Zebra in different orders.

**Mitigation:** Zalkanes derives canonical chain state from Zebra's reported
canonical tip and block hashes. Two nodes pointing to the same Zebra instance
will always see the same canonical chain. Nodes pointing to different Zebra
instances may briefly diverge during a reorg but will converge once both Zebra
instances agree.

### Replay of reorged-out deployment

**Attack:** A contract deployed in a reorged-out block is referenced by a
contract call in the new canonical chain.

**Mitigation:** After rollback, the ContractId of the reorged-out deployment is
removed from state. Any CALL referencing it in the new chain returns
"contract not found" and is ignored with no state mutation.

### State root mismatch after reorg replay

**Attack:** After rollback and replay, the state root differs from a fresh
index of the same chain.

**Mitigation:** The reorg test suite (spec §23) includes a test that compares
the post-reorg state root of a rolled-back node against a freshly indexed
node from the same blocks. They must be identical.

## Test coverage

Covered by REORG-001 through REORG-004 in spec §23, the crash consistency
suite (spec §24), and the three-node determinism test (spec §55).
