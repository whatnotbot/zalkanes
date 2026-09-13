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

/// Maximum number of Zalkanes protocol messages (OP_RETURN outputs) in one
/// Zcash transaction.
pub const MAX_ZALK_MESSAGES_PER_TX: u32 = 16;

/// Maximum number of Zalkanes protocol messages across one Zcash block.
pub const MAX_ZALK_MESSAGES_PER_BLOCK: u32 = 4_096;

/// Maximum total carrier bytes (deploy WASM + call calldata) processed across
/// one Zcash block.
pub const MAX_CARRIER_BYTES_PER_BLOCK: u64 = 4 * 1024 * 1024;

/// Maximum total deployment WASM bytes processed across one Zcash block.
pub const MAX_DEPLOY_BYTES_PER_BLOCK: u64 = 4 * 1024 * 1024;

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

// ── Protocol V1 (ADR-0008) ───────────────────────────────────────────────────
// Strictly additive; v0 messages keep their exact semantics forever.

/// Protocol version 1.
pub const PROTOCOL_V1: u8 = 0x01;

/// Message type: CALL_V1 (payload commitment; payload in P2SH carriers).
pub const MSG_CALL_V1: u8 = 0x04;

/// V1 mainnet activation height. MUST remain `None` (same audit gate as v0).
pub const V1_MAINNET_ACTIVATION_HEIGHT: Option<u32> = None;

/// V1 testnet activation height. None until a V1 RC freezes one.
pub const V1_TESTNET_ACTIVATION_HEIGHT: Option<u32> = None;

/// V1 regtest activation height (testing only).
pub const V1_REGTEST_ACTIVATION_HEIGHT: Option<u32> = Some(2);

/// Maximum assets attachable to one V1 CALL.
pub const MAX_V1_ATTACHED_ASSETS: u8 = 4;

/// Maximum V1 CALL carrier payload bytes (same cap as v0 carrier calldata).
pub const MAX_CALL_V1_PAYLOAD_BYTES: u32 = 65_536;

/// Maximum bytes of one contract event.
pub const MAX_EVENT_BYTES: u32 = 1_024;

/// Maximum events per top-level message.
pub const MAX_EVENTS_PER_CALL: u32 = 64;

/// V1 host-call fuel surcharges (ADR-0008 §17).
pub const FUEL_ASSET_OP: u64 = 5_000;
pub const FUEL_CONTRACT_CALL_BASE: u64 = 10_000;
pub const FUEL_CONTRACT_SPAWN: u64 = 50_000;
pub const FUEL_EVENT_BASE: u64 = 1_000;
pub const FUEL_EVENT_PER_BYTE: u64 = 10;
pub const FUEL_CONTEXT_OP: u64 = 100;

// ── Activation heights ───────────────────────────────────────────────────────

/// Mainnet activation height.
///
/// **MUST remain `None` until external audit approval.**
/// Setting this value activates the protocol on mainnet.
pub const MAINNET_ACTIVATION_HEIGHT: Option<u32> = None;

/// Testnet activation height.
///
/// **RC2.** Re-chosen for the fresh frozen-RC activation, because the previous
/// height (4,338,100) was frozen ~16 hours BEFORE the protocol itself was
/// frozen, so history above it was produced by pre-freeze tooling. See
/// `audit/TESTNET-ACTIVATION-AUDIT.md`.
///
/// Decision record (`audit/RC2-DECISION-RECORD.md`): measured against our own
/// Zebra full validator at 2026-09-12T11:26:32Z, canonical testnet tip 4,340,637
/// (`000071aa4d878924ea16a1d2bbf9d3352d982092378805b9ed37727ef5cd56d6`), giving
/// a lead of 5,863 blocks (~5.1 days at 75 s/block).
///
/// Strictly above every block that existed at freeze time, so no pre-existing
/// testnet history can be reinterpreted as protocol messages.
pub const TESTNET_ACTIVATION_HEIGHT: Option<u32> = Some(4_346_500);

/// Regtest activation height.
pub const REGTEST_ACTIVATION_HEIGHT: Option<u32> = Some(1);

// ── Blake2b personalization strings ─────────────────────────────────────────

/// Personalization for ContractId derivation. Exactly 16 bytes.
pub const CONTRACT_ID_PERSONALIZATION: &[u8; 16] = b"ZalkContractId0 ";

/// Personalization for state root leaf hashing. Exactly 16 bytes.
pub const STATE_LEAF_PERSONALIZATION: &[u8; 16] = b"ZalkStateLeaf0  ";

/// Personalization for state root hashing. Exactly 16 bytes.
pub const STATE_ROOT_PERSONALIZATION: &[u8; 16] = b"ZalkStateRoot0  ";

/// Personalization for V1 asset-ledger leaves. Exactly 16 bytes.
pub const ASSET_LEAF_PERSONALIZATION: &[u8; 16] = b"ZalkAssetLeaf1  ";

/// Personalization for V1 external account ids. Exactly 16 bytes.
pub const ACCOUNT_ID_PERSONALIZATION: &[u8; 16] = b"ZalkAccountId1  ";

/// Personalization for V1 spawned-contract ids. Exactly 16 bytes.
pub const SPAWN_ID_PERSONALIZATION: &[u8; 16] = b"ZalkSpawnId1    ";

/// Personalization for the V1 CALL auth sighash. Exactly 16 bytes.
pub const CALL_AUTH_PERSONALIZATION: &[u8; 16] = b"ZalkCallAuth1   ";
