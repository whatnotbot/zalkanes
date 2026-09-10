# ADR 0002 — Metashrew / Alkanes Reuse

**Status:** Accepted  
**Date:** 2025-09-10

## Context

Alkanes (kungfuflex/alkanes-rs) and Metashrew (kungfuflex/metashrew) implement
a Bitcoin-anchored WASM smart-contract metaprotocol. Zalkanes has the same
logical structure but anchors to Zcash. This ADR documents what can be reused,
what must be adapted, and what must be rewritten.

## Reuse matrix

| Component                  | Decision   | Reason                                                                                    |
|----------------------------|------------|-------------------------------------------------------------------------------------------|
| Alkanes contract ABI       | Adapt      | Keep opcode-dispatch pattern; remove Bitcoin-specific types (RuneId, AlkaneId assumptions) |
| Alkanes WASM runtime traits| Adapt      | Runtime trait patterns are reusable; fuel config and host ABI differ                     |
| Alkanes CallResponse       | Reuse      | Clean pattern with no Bitcoin-specific assumptions                                        |
| Alkanes balance model      | Adapt      | Port u128 balance logic; remove Protorunes/Runes mint logic                              |
| Alkanes contract macros    | Adapt      | Remove Bitcoin-specific derives; keep dispatch macro pattern                             |
| Alkanes test harness       | Adapt      | Replace Bitcoin block builder with Zcash block builder                                   |
| Protorunes                 | Rewrite    | Entirely Bitcoin-specific (Runes, Runestone, edict encoding)                             |
| Protostones                | Rewrite    | Bitcoin-specific protocol layer                                                           |
| Bitcoin tx parser          | Rewrite    | Use Zebra/librustzcash transparent tx types                                               |
| AlkaneId (Bitcoin encoding)| Rewrite    | ContractId uses ZIP-244 txid + output index + code_hash                                  |
| Metashrew state engine     | Adapt      | Port storage traits and state root; replace Bitcoin chain source                         |
| Metashrew chain sync       | Adapt      | Replace Bitcoin source with ZebraRpcChainSource; keep sync loop structure                |
| Metashrew reorg rollback   | Adapt      | Logic is chain-agnostic; replace block hash types                                        |
| Metashrew atomic commits   | Reuse      | RocksDB atomic batch pattern is directly reusable                                        |
| Metashrew state root       | Adapt      | Algorithm reused; personalization strings updated for Zalkanes                           |
| Wasmi integration          | Adapt      | Wasmi version pinned to =2.0.0; fuel config adjusted                                    |
| Zebra                      | Reuse      | Run as external process; consensus trust anchor; never fork                              |

## Bitcoin-specific dependency paths to eliminate

From alkanes-rs:
- `bitcoin` crate → remove entirely
- `ordinals` crate → remove entirely  
- `alkanes-support::proto::RuneId` → replaced by `ZalkContractId`
- All Protorunes message types → replaced by Zalkanes protocol messages
- `alkanes-runtime::AlkaneContext::rune_id` → replaced by `ZalkContext::contract_id`

From metashrew:
- `bitcoin::Block`, `bitcoin::Transaction` → replaced by Zcash/librustzcash types
- Rockshrew Bitcoin chain source → replaced by `ZebraRpcChainSource`

## Exact upstream commits studied

- alkanes-rs: `62511e9371a3f9e448841140c51cfe428cfcb955`
- metashrew: `3404d506ccc7517f733a566a9b9ad76d98bcbcb4`

## License compatibility

- alkanes-rs: MIT — compatible.
- metashrew: MIT — compatible.
- All adapted code must carry attribution comments.

## Consequences

- No direct crate dependencies on alkanes-rs or metashrew. Code is studied and adapted.
- All adapted modules must have attribution comments: `// Adapted from kungfuflex/alkanes-rs <commit> (MIT)`.
- Any divergence from Alkanes semantics must be documented in `docs/alkanes-compatibility.md`.
