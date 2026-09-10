# Protocol v0 (normative)

Canonical byte-exact specification: `docs/protocol-v0.md`.
Machine-readable manifest: `protocol/v0.toml` (hash exposed via
`zalkanes_getInfo.protocol_manifest_hash`).

Key points (see `docs/protocol-v0.md` for exact field layouts):

- OP_RETURN envelope: `MAGIC(4) || version(1) || message_type(1) || body`.
- Messages: DEPLOY (0x01), CALL_INLINE (0x02), CALL_CARRIER (0x03).
- Carrier redeem script: `0x21 <pubkey(33)> 0xac 0x61`
  (`<pubkey> OP_CHECKSIG OP_NOP`).
- Transactions: V5 + ZIP-244 txid/sighash (txid commits to effecting data,
  excludes scriptSigs).
- Hashes: `code_hash = SHA-256(wasm)`; `input_hash = SHA-256(calldata)`;
  `ContractId = BLAKE2b-256(personalization="ZalkContractId0 ", …)`.
