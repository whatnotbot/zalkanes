# AGENTS.md — Notes for AI Coding Agents

## Project summary

Zalkanes is a Zcash-native WASM smart-contract metaprotocol.
The first deliverable is a deterministic indexer: given Zcash blocks from a
local Zebra node, it produces a cryptographic state root that all honest nodes
agree on.

## Critical rules for agents

### Never do these

- Do not set a mainnet activation height. Leave it `None` / unset.
- Do not add floating git-branch dependencies (`branch = "main"`).
- Do not implement out-of-scope features (bridges, DEX, FROST, governance — see spec §51).
- Do not silently upgrade `wasmi`. Any Wasmi version change is a protocol change.
- Do not use JSON for consensus data encoding.
- Do not trust RPC responses without defensive parsing.
- Do not parallelize block execution unless you can prove sequential equivalence.
- Do not add `unsafe` without a security justification comment.

### Always do these

- Pin crate versions exactly in `Cargo.toml` and commit `Cargo.lock`.
- Add test vectors for any consensus-affecting code.
- Update `docs/upstream-lock.md` when a pinned dependency changes.
- Open an ADR (in `docs/adr/`) before changing parser, runtime, fuel, state root, or host ABI.
- Run `cargo test --workspace` and `cargo clippy --workspace --all-targets -- -D warnings` before finishing a task.

## Stop conditions

Stop and write an ADR rather than improvising if:

- Standard Zebra cannot relay the proposed carrier transaction
- WASM bytes cannot be recovered deterministically from chain data
- ZIP-244 txid/auth_digest behavior undermines deployment integrity
- Metashrew state root cannot be reused cleanly
- Fuel differs between x86_64 and ARM64
- Wasmi upgrade changes fixture fuel values
- State roots differ between machines

## Workspace layout

```
crates/zalkanes-core       consensus constants + shared types
crates/zalkanes-chain      ZebraRpcChainSource
crates/zalkanes-protocol   canonical binary parser
crates/zalkanes-carrier    P2SH carrier encode/decode
crates/zalkanes-runtime    Wasmi execution profile
crates/zalkanes-state      RocksDB state + state root
crates/zalkanes-indexer    block indexer + reorg
crates/zalkanes-rpc        JSON-RPC server
crates/zalkanes-sdk        contract SDK
crates/zalkanes-build      reproducible WASM build helpers
crates/zalkanes-tx         transaction construction
crates/zalkanes-testkit    in-memory test harness
crates/zalkanes-cli        CLI binary
contracts/counter          reference counter contract
contracts/key-value        reference KV contract
contracts/token            reference token contract
contracts/caller           reference cross-contract call demo
```

## Key documents

- `docs/protocol-v0.md` — canonical wire format (freeze before testnet)
- `docs/wasm-consensus.md` — Wasmi version + config (consensus-critical)
- `docs/upstream-lock.md` — pinned upstream versions
- `docs/adr/` — architectural decision records
- `test-vectors/` — immutable fixture files for CI

## PR sequence

PR-001 scaffolding → PR-002 chain source → PR-003 carrier spike →
PR-004 protocol parser → PR-005 WASM profile → PR-006 counter in memory →
PR-007 state engine → PR-008 block indexing → PR-009 reorg handling →
PR-010 regtest E2E → PR-011 SDK + CLI → PR-012 contract calls →
PR-013 token model → PR-014 RPC/views → PR-015 fuzzing →
PR-016 three-node determinism → PR-017 testnet RC
