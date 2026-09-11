# Upstream Lock

Records exact versions, commits, and consensus-critical status for every
external dependency.  Update this file whenever any entry changes and open a
protocol-version ADR if the dependency is marked **consensus-critical: yes**.

Last reviewed: 2025-09-10

---

## Rust toolchain

| Field          | Value                                      |
|----------------|--------------------------------------------|
| **Rust**       | **1.88.0**                                 |
| Pinned via     | `rust-toolchain.toml` (`channel = "1.88.0"`), `Cargo.toml` `rust-version`, Dockerfile `rust:1.88-slim-bookworm`, GitHub Actions `toolchain: 1.88.0` |
| Rationale      | transitive `icu_*` crates (via reqwest/jsonrpsee) require rustc ≥ 1.88 |
| Consensus-critical | no (toolchain only)                     |

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
| zcash_address       | 0.13.0    | no                | address encoding only                |

Repository: zcash/librustzcash

### V4 transparent sighash branch id (consensus-critical)

Zalkanes signs transparent-only **V4** transactions with the legacy ZIP-243
sighash. The sighash personalization embeds a **consensus branch id**, and the
validator recomputes that same id from the *network upgrade active at the
spending height* (Zebra reads a V4 transaction with
`NetworkUpgrade::current(network, height).branch_id()`).

Therefore the branch id is **per-network and height-dependent**, not a constant:

| Network          | Branch id   | Upgrade    |
|------------------|-------------|------------|
| regtest (Zebra)  | `0xE9FF75A6`| Canopy     |
| testnet (current)| `0x37A5165B`| Nu6.3      |
| mainnet          | *(unset)*   | pre-audit  |

`zalkanes-tx::branch_id_for_network` pins this mapping and is exercised by the
testnet acceptance evidence. Signing with the wrong branch id makes every
transparent signature invalid under consensus. Testnet tip (2026-09-10) is
~4,338,009 blocks, past the Nu6.3 activation height 4,134,000.

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

## Serialization / utility (non-consensus)

| Crate  | Version | Notes                                                              |
|--------|---------|--------------------------------------------------------------------|
| serde  | =1.0.220 | bumped from =1.0.210 so the `time` >=0.3.47 advisory fix (RUSTSEC-2026-0009, via `rusqlite`/`zcash_client_sqlite`) resolves; `time 0.3.47+` requires `serde_core` 1.0.220 |
| time   | 0.3.55 (lock) | transitive (rusqlite); bumped to clear RUSTSEC-2026-0009 (RFC 2822 stack-exhaustion DoS) |

---

## Protocol constants

```
PROTOCOL_MAGIC  = [0x5A, 0x41, 0x4C, 0x4B]   // "ZALK"
PROTOCOL_V0     = 0x00
```

Mainnet activation height: **UNSET — requires external audit approval before setting.**
Testnet activation height: defined in `zalkanes-core/src/consensus.rs`.
