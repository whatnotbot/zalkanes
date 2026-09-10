# ADR 0004 — Contract Identity

**Status:** Accepted  
**Date:** 2025-09-10

## Context

Each deployed contract needs a deterministic, immutable, globally unique
identifier. The identifier must be derivable by any node from on-chain data
alone, with no trusted party.

## Decision

```
ContractId = BLAKE2b-256(
    personalization = b"ZalkContractId0 ",   // 16 bytes exactly
    input = network_id (1 byte)
          || txid (32 bytes, internal byte order)
          || output_index (2 bytes, big-endian u16)
          || code_hash (32 bytes, SHA-256 of WASM)
)
```

Total input: 67 bytes.

### Field encoding

| Field          | Type        | Bytes | Notes                                        |
|----------------|-------------|-------|----------------------------------------------|
| network_id     | u8          | 1     | 0x01=mainnet, 0x02=testnet, 0x03=regtest     |
| txid           | [u8; 32]    | 32    | ZIP-244 txid, internal byte order (as stored in block) |
| output_index   | u16 big-end | 2     | index of the OP_RETURN output                |
| code_hash      | [u8; 32]    | 32    | SHA-256(raw_wasm_bytes)                      |

### Personalization string

`ZalkContractId0 ` — exactly 16 bytes, padded with one ASCII space.
The trailing digit allows future versions to produce different identifiers if the algorithm changes.

## Rationale

- BLAKE2b is used throughout the Zcash ecosystem (ZIP-244, note commitment).
- Including `code_hash` means two deployments of the same code produce different `ContractId`s (different txids), which is correct.
- Including `network_id` prevents mainnet/testnet ContractId collisions.
- Using the ZIP-244 `txid` (effecting data hash) ensures the identifier is non-malleable.
- Internal byte order matches how txids are stored and displayed in Zcash/Zebra.

## Test vectors

File: `test-vectors/protocol/contract-id-v0.json`

Each vector:
```json
{
  "network": "regtest",
  "txid_hex": "...",
  "output_index": 1,
  "code_hash_hex": "...",
  "expected_contract_id_hex": "..."
}
```

Two independent implementations given the same inputs MUST produce the same ContractId.

## Consequences

- ContractId is 32 bytes.
- Upgradeability is at the contract level (proxy pattern), not protocol level.
- Protocol-level mutable code replacement is NOT implemented in v0.
- The ContractId algorithm is a protocol constant — changing it requires a new protocol version.
