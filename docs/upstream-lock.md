# Upstream Lock

Records exact versions, commits, and consensus-critical status for every
external dependency.  Update this file whenever any entry changes and open a
protocol-version ADR if the dependency is marked **consensus-critical: yes**.

Last reviewed: 2025-09-10

---

## Zcash consensus node

| Field                  | Value                                          |
|------------------------|------------------------------------------------|
| Repository             | ZcashFoundation/zebra                          |
| **ZEBRA_VERSION**      | **v6.3.0**                                     |
| **ZEBRA_COMMIT**       | **9d527513eb394162e2735ee8a3f0592b405bead0**   |
| **ZCASH_NETWORK_UPGRADE** | **Nu6 (Zclimate)**                          |
| Consensus-critical     | yes — trust anchor for all block data          |
| Notes                  | Zalkanes MUST NOT maintain a consensus fork.   |

---

## Zcash Rust libraries (librustzcash)

| Crate               | Version   | Consensus-critical | Notes                                |
|---------------------|-----------|-------------------|--------------------------------------|
| zcash_primitives    | 0.30.1    | yes               | transaction types / data structures  |
| zcash_protocol      | 0.10.6    | yes               | network constants / note encoding    |
| zcash_transparent   | 0.10.0    | yes               | transparent script / UTXO types      |
| zcash_address       | 0.6.0     | no                | address encoding only                |

Repository: zcash/librustzcash

---

## WASM runtime

| Field              | Value                                         |
|--------------------|-----------------------------------------------|
| Crate              | wasmi                                         |
| **Pinned version** | **=2.0.0**                                    |
| Repository         | wasmi-labs/wasmi                              |
| Consensus-critical | **YES — changing this requires new protocol version + fuel re-vectors** |
| Enabled features   | no-std, metered fuel                          |
| Forbidden features | floats (disabled at module validation), threads, SIMD, memory64 |
| Notes              | Cargo.toml pins `=2.0.0`. Cargo.lock is committed. |

---

## Alkanes reference implementation

| Field          | Value                                              |
|----------------|----------------------------------------------------|
| Repository     | kungfuflex/alkanes-rs                              |
| Commit studied | 62511e9371a3f9e448841140c51cfe428cfcb955           |
| Reuse scope    | runtime traits, ABI patterns, CallResponse, SDK macros |
| NOT reused     | Runes, Protorunes, Protostones, Bitcoin tx parser  |
| Deprecated ref | kungfuflex/alkanes — reference-only, not a dependency |

---

## Metashrew

| Field          | Value                                              |
|----------------|----------------------------------------------------|
| Repository     | kungfuflex/metashrew                               |
| Commit studied | 3404d506ccc7517f733a566a9b9ad76d98bcbcb4           |
| Reuse scope    | state engine traits, state root design, reorg/rollback patterns, atomic block commits |
| NOT reused     | Bitcoin chain source (replaced by ZebraRpcChainSource) |

---

## Storage

| Crate   | Version | Consensus-critical | Notes                        |
|---------|---------|--------------------|------------------------------|
| rocksdb | =0.22.0 | no                 | state persistence only; varying cache size must not affect state roots |

---

## Protocol constants

```
PROTOCOL_MAGIC  = [0x5A, 0x41, 0x4C, 0x4B]   // "ZALK"
PROTOCOL_V0     = 0x00
```

Mainnet activation height: **UNSET — requires external audit approval before setting.**
Testnet activation height: defined in `zalkanes-core/src/consensus.rs`.
