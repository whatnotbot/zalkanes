# Consensus-Critical Files

Any modification to the following files can change message validity,
`ContractId`, WASM behavior, fuel, state, the state root, reorg behavior, or
activation — and therefore MUST be accompanied by a protocol-version/ADR change
and new test vectors.

| File | Why consensus-critical |
|------|------------------------|
| `protocol/v0.toml` | Machine-readable manifest; its hash is the protocol fingerprint |
| `crates/zalkanes-core/src/consensus.rs` | All constants: limits, magic, message types, activation heights, personalization |
| `crates/zalkanes-core/src/consensus_params.rs` | Height-aware consensus branch-id resolution |
| `crates/zalkanes-core/src/manifest.rs` | Manifest embedding + hash |
| `crates/zalkanes-core/src/types.rs` | `ContractId` derivation (network id + txid + output index + code hash) |
| `crates/zalkanes-protocol/src/lib.rs` | OP_RETURN message byte-exact parser/encoder |
| `crates/zalkanes-carrier/src/lib.rs` | Carrier chunk reconstruction + hash/length verification |
| `crates/zalkanes-tx/src/lib.rs` | V5/ZIP-244 tx construction; carrier redeem script; chunk payload size; fees |
| `crates/zalkanes-runtime/src/lib.rs` | Wasmi config, fuel, host ABI, storage write cap, module validation |
| `crates/zalkanes-state/src/lib.rs` | State root hashing, atomic commit, rollback journal |
| `crates/zalkanes-indexer/src/lib.rs` | Block processing, deploy/call discovery, reorg |
| `crates/zalkanes-indexer/src/parse.rs` | Real Zcash block/transaction parsing |

## ContractId (canonical)

```
ContractId = BLAKE2b-256(
    personalization = "ZalkContractId0 ",
    input = network_id (u8)
          || txid (32 bytes, internal byte order)
          || output_index (u16 BE)
          || code_hash (32 bytes)
)
```

`network_id`: 0x01 mainnet, 0x02 testnet, 0x03 regtest. `txid` is the ZIP-244
txid (effecting-data hash) for V5 transactions.

## State root (canonical)

```
leaf = BLAKE2b-256(personalization="ZalkStateLeaf0  ", input)
root = BLAKE2b-256(personalization="ZalkStateRoot0  ", sorted(leaves) concatenated)
```

- Contract leaf input = `contract_id (32) || code_hash (32) || code_len (u32 BE) || wasm`.
- Storage leaf input = `contract_id (32) || key_len (u16 BE) || key || value_len (u32 BE) || value`.
- Leaves are sorted lexicographically by their 32-byte digest before the root
  hash. Empty root = hash of zero leaves. See `docs/adr/0006-state-root.md` and
  `tools/zalkanes-reference/`.
