# Protocol v0

> **Status: DRAFT — do not freeze until Phase 0B carrier spike passes.**

This document specifies the exact binary wire format for all Zalkanes v0
protocol messages.  Every parser MUST implement this spec byte-for-byte.
Any ambiguity is a bug in the spec, not a parser option.

---

## 1. Constants

```
MAGIC           = [0x5A, 0x41, 0x4C, 0x4B]   // "ZALK"  big-endian
PROTOCOL_V0     = 0x00
```

---

## 2. Carrier: OP_RETURN envelope

Every Zalkanes protocol message is announced by an OP_RETURN output in a
Zcash transparent transaction.

```
OP_RETURN payload layout (bytes):

 0..3    magic           4 bytes   0x5A 0x41 0x4C 0x4B
 4       version         1 byte    0x00 for v0
 5       message_type    1 byte    see §3
 6..     message_body    variable  see §4, §5
```

Rules:
- Payload MUST begin with the 4-byte magic.
- Unknown version: REJECT (do not apply state).
- Unknown message_type: REJECT.
- Trailing bytes after the message_body MUST cause rejection.
- Maximum OP_RETURN payload: 80 bytes (Zcash standard policy).

---

## 3. Message types

```
DEPLOY       = 0x01
CALL_INLINE  = 0x02
CALL_CARRIER = 0x03
```

---

## 4. Integer encoding

All integers in Zalkanes protocol messages are encoded as:

- **u8**:  1 byte, unsigned.
- **u16**: 2 bytes, big-endian, unsigned.
- **u32**: 4 bytes, big-endian, unsigned.
- **u64**: 8 bytes, big-endian, unsigned.

No varint encoding.  No platform-dependent width.
No serde integer defaults.

---

## 5. DEPLOY message body

```
Field           Type    Bytes   Description
─────────────────────────────────────────────────────────
code_hash       bytes   32      SHA-256 of the WASM bytes
code_length     u32     4       exact byte length of WASM
chunk_count     u8      1       number of P2SH carrier inputs
output_index    u16     2       output index of this OP_RETURN
```

Total DEPLOY body: 39 bytes.
Total OP_RETURN payload with header: 6 + 39 = 45 bytes.

Validation:
- `code_length` MUST be > 0 and <= MAX_CODE_BYTES (262144).
- `chunk_count` MUST be > 0 and <= MAX_CHUNKS (188).
- `output_index` MUST match the actual output index of this OP_RETURN.

---

## 6. CALL messages

A CALL delivers method input to a deployed contract. Two encodings exist; the
parser distinguishes them by `message_type`.

### 6.1 CALL_INLINE (message_type = 0x02)

Small inputs that fit entirely in the OP_RETURN payload.

```
Field           Type    Bytes   Description
─────────────────────────────────────────────────────────
contract_id     bytes   32      ContractId (see §8)
opcode          u16     2       method selector
input_length    u16     2       byte length of input data
input           bytes   var     contract input; 0..MAX_CALL_INLINE_BYTES
```

Total CALL body (excluding input): 36 bytes.
Total OP_RETURN payload with header: 6 + 36 + input.

`MAX_CALL_INLINE_BYTES = 80 - 6 - 32 - 2 - 2 = 38`.

Validation:
- `contract_id` MUST reference a deployed contract.
- `input_length` MUST equal actual `input` length.
- `input_length` MUST be <= MAX_CALL_INLINE_BYTES (38).
- Trailing bytes after `input[input_length]`: REJECT.

### 6.2 CALL_CARRIER (message_type = 0x03)

Large inputs (up to MAX_CALL_INPUT_BYTES) are delivered through the same
authenticated P2SH carrier used for DEPLOY (§7). The OP_RETURN carries the
calldata commitment; the calldata itself rides in carrier scriptSigs.

```
Field           Type    Bytes   Description
─────────────────────────────────────────────────────────
contract_id     bytes   32      ContractId (see §8)
opcode          u16     2       method selector
input_hash      bytes   32      SHA-256 of the exact calldata bytes
input_length    u32     4       exact byte length of calldata
carrier_count   u8      1       number of P2SH carrier inputs
```

Total CALL_CARRIER body: 71 bytes.
Total OP_RETURN payload with header: 6 + 71 = 77 bytes.

Validation:
- `contract_id` MUST reference a deployed contract.
- `input_length` MUST be > MAX_CALL_INLINE_BYTES and <= MAX_CALL_INPUT_BYTES.
- `carrier_count` MUST be > 0 and <= MAX_CALL_CARRIER_CHUNKS.
- Reconstructed calldata MUST satisfy `SHA-256(calldata) == input_hash` and
  `len(calldata) == input_length` before execution.

`MAX_CALL_INPUT_BYTES = 65536` (64 KiB); `MAX_CALL_CARRIER_CHUNKS = ceil(65536 / 1400) = 47`.

A failed CALL_CARRIER (bad hash, bad length, missing/duplicate/out-of-range
chunk) is rejected with no state mutation, exactly like a failed DEPLOY.

---

## 7. Carrier: P2SH chunk encoding

WASM bytes are split across P2SH carrier inputs in the DEPLOY transaction.

Each carrier input's scriptSig contains:

```
<chunk_index: u8>  <chunk_data: bytes>  <signature>  <redeem_script>
```

Reconstruction:
1. Collect all scriptSig inputs tagged with chunk indexes 0..chunk_count-1.
2. Sort by chunk_index ascending.
3. Concatenate chunk_data in order.
4. ASSERT: len(concatenated) == code_length from DEPLOY message.
5. ASSERT: SHA-256(concatenated) == code_hash from DEPLOY message.

If any chunk is missing, duplicated, or out of range: REJECT.
Reconstruction is deterministic regardless of input ordering in the transaction.

Redeem script (P2SH) — the **exact** 36 bytes accepted on regtest and public
testnet (no "conceptual" scripts):

```
0x21 <deployer_pubkey(33 bytes)> 0xac 0x61
```

i.e. `<deployer_pubkey> OP_CHECKSIG OP_NOP`. The trailing `OP_NOP` makes the
script non-standard as a solver template, which is what permits the extra
`chunk_index`/`chunk_data` pushes to remain below the final `true` value (see
ADR-0003). The carrier UTXO is not anyone-can-spend; a different key cannot
spend it (Zcash script validation enforces this).

---

## 8. ContractId

```
ContractId = BLAKE2b-256(
    personalization = "ZalkContractId0 ",   // exactly 16 bytes
    input = network_id (1 byte)
          || txid (32 bytes, internal byte order)
          || output_index (2 bytes, big-endian)
          || code_hash (32 bytes)
)
```

- `network_id`: 0x01 = mainnet, 0x02 = testnet, 0x03 = regtest.
- `txid`: ZIP-244 txid (effecting data hash), internal byte order (as stored in blocks).
- `output_index`: index of the OP_RETURN output.
- `code_hash`: SHA-256 of raw WASM bytes.

Two implementations given the same inputs MUST produce the same ContractId.
Test vectors: `test-vectors/protocol/contract-id-v0.json`.

---

## 9. Failure behavior

On any parse error or validation failure:

- The message is ignored.
- No state is modified.
- The node continues processing the remainder of the block.
- The failure is logged at DEBUG level with the txid.

---

## 10. Unknown version / opcode

- Unknown version field (byte 4): entire payload ignored.
- Unknown message_type (byte 5): entire payload ignored.
- Both cases: no state mutation, no panic.

---

## 11. ZIP-244 note

For Zcash v5+ transactions, `txid` is the non-malleable transaction identifier
(hash of effecting data).  Authorization data (scriptSigs) is NOT part of txid.

The `code_hash` in the DEPLOY message therefore commits the effecting data to
the exact WASM payload.  Any mutation of carrier scriptSig bytes that changes
chunk data will cause `SHA-256(reconstructed) != code_hash` and REJECT.

---

## 12. Consensus limits (v0 defaults)

See `crates/zalkanes-core/src/consensus.rs` for authoritative values.

```
MAX_CODE_BYTES              = 262_144           // 256 KiB
MAX_CHUNKS                  = 188               // carrier chunks per deploy
CHUNK_PAYLOAD_SIZE          = 1_400             // bytes per carrier scriptSig
MAX_CALL_INLINE_BYTES       = 38                // inline CALL input
MAX_CALL_INPUT_BYTES        = 65_536            // 64 KiB (carrier CALL)
MAX_CALL_CARRIER_CHUNKS     = 47                // ceil(65536 / 1400)
MAX_RETURN_DATA_BYTES       = 65_536
MAX_STORAGE_KEY_BYTES       = 256
MAX_STORAGE_VALUE_BYTES     = 65_536
MAX_STORAGE_WRITES_PER_CALL = 256
MAX_FUEL_PER_CALL           = 10_000_000
MAX_FUEL_PER_ZCASH_TX       = 100_000_000
MAX_FUEL_PER_ZCASH_BLOCK    = 1_000_000_000
MAX_CALL_DEPTH              = 16
MAX_LINEAR_MEMORY_PAGES     = 256              // 16 MiB
MAX_TABLE_ELEMENTS          = 16_384
MAX_GLOBALS                 = 512
MAX_FUNCTIONS               = 4_096
MAX_IMPORTS                 = 64
MAX_EXPORTS                 = 64
```
