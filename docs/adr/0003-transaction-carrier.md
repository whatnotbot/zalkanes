# ADR 0003 — Transaction Carrier Design

**Status:** Draft — pending Phase 0B spike results  
**Date:** 2025-09-10

## Context

Zcash standard OP_RETURN outputs are limited to 80 bytes. WASM binaries for
non-trivial contracts easily exceed this. The WASM bytes must be recoverable
entirely from Zcash blockchain data with no external dependencies (no IPFS,
Arweave, HTTP, CDN, or project API).

## Candidate design: two-stage P2SH carrier

### Stage 1 — PREPARE transaction

The deployer constructs one or more P2SH carrier UTXOs. Each UTXO's redeem
script is:

```
<deployer_pubkey> OP_CHECKSIG
```

This is not anyone-can-spend. A different key cannot spend these UTXOs (Zcash
script validation enforces this — see CARRIER-008 test).

### Stage 2 — DEPLOY transaction

The DEPLOY transaction:
1. Spends the carrier UTXOs. Each `scriptSig` pushes:
   - `<chunk_index: u8>`
   - `<chunk_data: bytes>`
   - `<signature>`
   - `<redeem_script>`
2. Includes an OP_RETURN output with the Zalkanes DEPLOY message:
   - ZALK magic (4 bytes)
   - Protocol version (1 byte)
   - DEPLOY type (1 byte)
   - `code_hash` = SHA-256(wasm_bytes) (32 bytes)
   - `code_length` = u32 (4 bytes)
   - `chunk_count` = u8 (1 byte)
   - `output_index` = u16 (2 bytes)

### Reconstruction

Given the DEPLOY transaction on-chain:
1. Find the OP_RETURN with ZALK magic.
2. Parse `code_hash`, `code_length`, `chunk_count`.
3. Collect `chunk_count` scriptSig inputs, sorted by `chunk_index`.
4. Concatenate chunk data.
5. Assert: `len(concat) == code_length`.
6. Assert: `SHA-256(concat) == code_hash`.

If either assertion fails: reject deployment, no state mutation.

## ZIP-244 commitment

In Zcash v5+ transactions, `txid` covers only effecting data. Authorization
data (scriptSigs) is covered by `auth_digest` and is NOT part of `txid`.

The `code_hash` in the OP_RETURN output IS part of effecting data (txid). Any
mutation of carrier bytes that changes chunk content will cause
`SHA-256(reconstructed) != code_hash` and be rejected.

This provides cryptographic commitment of effecting data to the exact WASM
payload. See CARRIER-009 test for the ZIP-244 fixture.

## Open questions (spike must answer)

1. Does a standard Zebra regtest mempool relay a DEPLOY transaction with
   multiple P2SH inputs and an OP_RETURN? (CARRIER-010)
2. What is the maximum practical payload size within default Zebra relay policy?
   (CARRIER-002, CARRIER-003)
3. Does the number of inputs required for 256 KiB payloads exceed any Zebra
   policy limit?

## Stop condition

If standard Zebra cannot relay the proposed carrier transaction, this ADR must
be updated with an alternative design before any implementation continues.

## Acceptance criteria

All tests CARRIER-001 through CARRIER-011 must pass before this ADR is marked Accepted.

## Consequences

- Two on-chain transactions required for every deployment (PREPARE + DEPLOY).
- Deployer must hold ZEC to fund both transactions and pay ZIP-317 fees.
- WASM binary size is bounded by Zebra relay policy (to be determined by spike).
- Chunk ordering is explicit via `chunk_index`; no dependence on hash-map iteration.
