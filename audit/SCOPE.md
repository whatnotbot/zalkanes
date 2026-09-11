# Audit Scope

## In scope

1. **Protocol wire format** — `docs/protocol-v0.md` and `protocol/v0.toml`
   (OP_RETURN messages, carrier encoding, integer/hash encodings).
2. **Transaction construction** — `crates/zalkanes-tx`: V5/ZIP-244 PREPARE,
   DEPLOY, CALL_INLINE, CALL_CARRIER construction, signing, and fee logic.
3. **Parser + carrier reconstruction** — `crates/zalkanes-protocol`,
   `crates/zalkanes-carrier`: byte-exact decoding of messages and chunks.
4. **Execution runtime** — `crates/zalkanes-runtime`: Wasmi 2.0.0 configuration,
   fuel metering, host ABI, storage write buffering, trap handling.
5. **State + state root** — `crates/zalkanes-state`: BLAKE2b state root, atomic
   commit, rollback journal.
6. **Indexer** — `crates/zalkanes-indexer`: block/transaction parsing, message
   discovery, reorg handling, pre-activation fast-forward.
7. **Activation + branch handling** — `crates/zalkanes-core`: activation heights
   and height-aware consensus branch-id resolution.
8. **Determinism** — cross-platform state-root equality and the reference
   implementation (`tools/zalkanes-reference`).

## Out of scope

- Application-layer features (no DEX/bridge/FROST/governance/tokens).
- Mainnet activation (`MAINNET_ACTIVATION_HEIGHT` is `None`).
- Performance/SLO tuning.
- P2P/network-layer security of the Zebra node itself (Zebra is upstream).
- Key custody / deployment security beyond what the threat model states.

## Security invariants (must hold)

- A Zalkanes message is either byte-exact valid or is ignored with **no state
  mutation** and **no panic**.
- The state root is a pure function of the ordered Zcash chain data.
- Contract execution is **atomic**: a trap/fuel-exhaustion discards all writes.
- The indexer never diverges from the state implied by the canonical chain.
