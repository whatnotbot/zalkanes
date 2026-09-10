//! Consensus constants for Zalkanes protocol v0.
//!
//! **All values are consensus-critical.**
//! Changing any value requires a new protocol version, updated ADR, and
//! new test vectors.

// ── WASM module limits ───────────────────────────────────────────────────────

/// Maximum WASM bytecode size for a deployed contract.
///
/// 256 KiB (262,144 bytes). This is the largest size representable by the
/// carrier: `MAX_CHUNKS (188) × CHUNK_PAYLOAD_SIZE (1400) = 263,200 ≥ 262,144`,
/// and `chunk_index`/`chunk_count` are `u8`. 256 KiB has been empirically
/// relayed on live Zebra. A 512 KiB constant would NOT be encodable (255 × 1400
/// = 357,000 < 524,288), so it must not be used.
pub const MAX_CODE_BYTES: u32 = 262_144; // 256 KiB

/// Maximum number of carrier chunks for one contract.
///
/// `ceil(MAX_CODE_BYTES / CHUNK_PAYLOAD_SIZE) = ceil(262144 / 1400) = 188`.
pub const MAX_CHUNKS: u8 = 188;

/// Maximum WASM linear memory pages (1 page = 64 KiB).
pub const MAX_LINEAR_MEMORY_PAGES: u32 = 256; // 16 MiB

/// Maximum WASM table elements.
pub const MAX_TABLE_ELEMENTS: u32 = 16_384;

/// Maximum number of globals in a WASM module.
pub const MAX_GLOBALS: u32 = 512;

/// Maximum number of functions in a WASM module.
pub const MAX_FUNCTIONS: u32 = 4_096;

/// Maximum number of imports in a WASM module.
pub const MAX_IMPORTS: u32 = 64;

/// Maximum number of exports in a WASM module.
pub const MAX_EXPORTS: u32 = 64;

// ── Execution limits ─────────────────────────────────────────────────────────

/// Maximum fuel per single contract call.
pub const MAX_FUEL_PER_CALL: u64 = 10_000_000;

/// Maximum total fuel across all calls within a single Zcash transaction.
pub const MAX_FUEL_PER_ZCASH_TX: u64 = 100_000_000;

/// Maximum total fuel across all calls within a single Zcash block.
pub const MAX_FUEL_PER_ZCASH_BLOCK: u64 = 1_000_000_000;

/// Maximum contract-to-contract call depth.
pub const MAX_CALL_DEPTH: u32 = 16;

// ── Storage limits ───────────────────────────────────────────────────────────

/// Maximum byte length of a storage key.
pub const MAX_STORAGE_KEY_BYTES: u32 = 256;

/// Maximum byte length of a storage value.
pub const MAX_STORAGE_VALUE_BYTES: u32 = 65_536; // 64 KiB

/// Maximum storage writes per call (including nested calls).
pub const MAX_STORAGE_WRITES_PER_CALL: u32 = 256;

// ── I/O limits ───────────────────────────────────────────────────────────────

/// Maximum byte length of inline (OP_RETURN-embedded) CALL input.
///
/// `80 (OP_RETURN policy max) - 6 (header) - 32 (contract_id) - 2 (opcode)
/// - 2 (input_length) = 38`.
pub const MAX_CALL_INLINE_BYTES: u32 = 38;

/// Maximum byte length of carrier-delivered CALL input.
///
/// 64 KiB, delivered via the P2SH carrier (see `MAX_CALL_CARRIER_CHUNKS`).
pub const MAX_CALL_INPUT_BYTES: u32 = 65_536; // 64 KiB

/// Maximum number of carrier inputs for a carrier CALL.
///
/// `ceil(MAX_CALL_INPUT_BYTES / CHUNK_PAYLOAD_SIZE) = ceil(65536 / 1400) = 47`.
pub const MAX_CALL_CARRIER_CHUNKS: u8 = 47;

/// Maximum byte length of call return data.
pub const MAX_RETURN_DATA_BYTES: u32 = 65_536; // 64 KiB

// ── Protocol wire format ─────────────────────────────────────────────────────

/// Magic bytes identifying a Zalkanes protocol message in OP_RETURN.
pub const PROTOCOL_MAGIC: [u8; 4] = [0x5A, 0x41, 0x4C, 0x4B]; // "ZALK"

/// Protocol version 0.
pub const PROTOCOL_V0: u8 = 0x00;

/// Message type: DEPLOY.
pub const MSG_DEPLOY: u8 = 0x01;

/// Message type: CALL (inline input, embedded in OP_RETURN).
pub const MSG_CALL: u8 = 0x02;

/// Message type: CALL_CARRIER (large input delivered via P2SH carrier).
pub const MSG_CALL_CARRIER: u8 = 0x03;

// ── Activation heights ───────────────────────────────────────────────────────

/// Mainnet activation height.
///
/// **MUST remain `None` until external audit approval.**
/// Setting this value activates the protocol on mainnet.
pub const MAINNET_ACTIVATION_HEIGHT: Option<u32> = None;

/// Testnet activation height.
///
/// Frozen immediately before the controlled first testnet deployment (external
/// testnet tip was 4,338,016 blocks, branch id `0x37a5165b` = Nu6.3, at freeze
/// time). Chosen just above the current tip so Zalkanes never interprets
/// arbitrary pre-activation testnet history as protocol messages.
pub const TESTNET_ACTIVATION_HEIGHT: Option<u32> = Some(4_338_100);

/// Regtest activation height.
pub const REGTEST_ACTIVATION_HEIGHT: Option<u32> = Some(1);

// ── Blake2b personalization strings ─────────────────────────────────────────

/// Personalization for ContractId derivation. Exactly 16 bytes.
pub const CONTRACT_ID_PERSONALIZATION: &[u8; 16] = b"ZalkContractId0 ";

/// Personalization for state root leaf hashing. Exactly 16 bytes.
pub const STATE_LEAF_PERSONALIZATION: &[u8; 16] = b"ZalkStateLeaf0  ";

/// Personalization for state root hashing. Exactly 16 bytes.
pub const STATE_ROOT_PERSONALIZATION: &[u8; 16] = b"ZalkStateRoot0  ";
