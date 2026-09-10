# Architecture

## Overview

Zalkanes is a metaprotocol indexer that interprets Zcash transparent transactions as WASM smart-contract operations and derives a deterministic state root.

```
         Zcash P2P
             │
             ▼
     ┌───────────────┐
     │     Zebra     │  local, consensus-validated
     └───────┬───────┘
             │  raw validated blocks (JSON-RPC)
             ▼
     ┌───────────────────────────────────────────┐
     │                 Zalkanes                  │
     │                                           │
     │  zalkanes-chain   ← ZebraRpcChainSource   │
     │  zalkanes-protocol← canonical parser      │
     │  zalkanes-carrier ← P2SH carrier decode   │
     │  zalkanes-runtime ← Wasmi profile         │
     │  zalkanes-state   ← RocksDB + state root  │
     │  zalkanes-indexer ← block loop + reorgs   │
     │  zalkanes-rpc     ← JSON-RPC server       │
     └───────────────────────────────────────────┘
```

## Crate responsibilities

### zalkanes-core
Consensus constants, shared types (`ContractId`, `CodeHash`, `BlockRef`),
and the `Network` enum. No external I/O. Every other crate depends on this.

### zalkanes-chain
`ChainSource` trait + `ZebraRpcChainSource` implementation.
Translates Zebra JSON-RPC responses into raw block bytes.
Performs defensive parsing of all RPC data.

### zalkanes-protocol
Canonical binary parser and serializer for all v0 protocol messages
(`DEPLOY`, `CALL`). Reads OP_RETURN payloads.
Deterministic: no serde defaults, no map iteration, no platform-dependent types.

### zalkanes-carrier
P2SH carrier encoding/decoding.
Reads chunk data from transparent scriptSig inputs.
Validates against committed code hash in OP_RETURN.

### zalkanes-runtime
Wasmi 2.0.0 execution profile.
Validates WASM modules against the v0 feature set.
Provides the host ABI (`storage_get`, `storage_set`, etc.).
Manages fuel budget and trap semantics.

### zalkanes-state
RocksDB-backed state engine.
Atomic block commits.
State root computation (Blake2b Merkle over sorted contract storage).
Historical state for reorg rollback.

### zalkanes-indexer
Block processing loop.
Calls `ChainSource`, parses blocks, dispatches to runtime, commits state.
Handles reorganizations (rollback to common ancestor, replay).
Crash-safe: every commit is atomic.

### zalkanes-rpc
JSON-RPC 2.0 server (jsonrpsee).
Exposes `zalkanes_getInfo`, `zalkanes_view`, etc.
Never exposes internal state engine directly; reads through a read-only handle.

### zalkanes-sdk
Contract-side SDK: `#[zalkanes::contract]` macro, storage helpers,
`CallResponse`, context access. Targets `wasm32-unknown-unknown`.

### zalkanes-build
Reproducible WASM build helpers.
`cargo build --target wasm32-unknown-unknown` wrapper with pinned flags.

### zalkanes-tx
Transparent transaction construction.
ZIP-317 fee calculation.
Does NOT sign; integrates with wallet via PSBT-style flow.

### zalkanes-testkit
In-memory test harness.
`TestChain::new()` → `deploy()` → `call()` → `view()`.
Uses real protocol parser, real Wasmi, real state root.

### zalkanes-cli
`zalkanes` binary. Subcommands: `node`, `contract`, `state-root`, `trace`.

## Data flow: DEPLOY

```
Zcash block arrives at indexer
  │
  ├─ zalkanes-protocol parses OP_RETURN → Deployment { code_hash, carrier_meta }
  ├─ zalkanes-carrier reconstructs WASM bytes from scriptSig chunks
  ├─ HASH(wasm_bytes) verified == code_hash
  ├─ zalkanes-runtime validates WASM module
  ├─ ContractId = H(network || txid || output_index || code_hash)
  ├─ zalkanes-state stores (ContractId → code, metadata)
  └─ state root updated
```

## Data flow: CALL

```
Zcash block arrives at indexer
  │
  ├─ zalkanes-protocol parses OP_RETURN → Call { contract_id, opcode, input }
  ├─ zalkanes-runtime executes contract WASM with fuel limit
  ├─ Atomic overlay: on success → commit; on trap/fuel → discard
  └─ state root updated
```

## Reorg handling

```
Canonical chain: A-B-C-D
New canonical:   A-B-X-Y-Z

1. find common ancestor B
2. rollback state to height B (using stored rollback data)
3. verify state root at B matches stored root
4. apply X, Y, Z sequentially
```

## State root

```
state_root = Blake2b(
  sorted [
    H(contract_id || storage_key || storage_value)
    for all (contract_id, key, value) in state
  ]
)
```

Exact algorithm specified in `docs/state-model.md` and frozen in `test-vectors/state-roots/`.
