//! Core types shared across all Zalkanes crates.

use serde::{Deserialize, Serialize};

/// 32-byte Zcash block hash.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct BlockHash(pub [u8; 32]);

/// Zcash block height.
pub type BlockHeight = u32;

/// ZIP-244 transaction identifier (32 bytes, internal byte order).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct TxId(pub [u8; 32]);

/// SHA-256 hash of raw WASM bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CodeHash(pub [u8; 32]);

/// Deterministic contract identifier (BLAKE2b-256).
/// See ADR 0004 and `docs/protocol-v0.md §8`.
/// `Ord` is byte-lexicographic (like `TxId`) so the id can key ordered
/// maps; it has no consensus meaning.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ContractId(pub [u8; 32]);

/// Zalkanes state root (BLAKE2b-256 over sorted state leaves).
/// See ADR 0006.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct StateRoot(pub [u8; 32]);

/// A reference to a specific block by height and hash.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockRef {
    pub height: BlockHeight,
    pub hash: BlockHash,
}

/// The result of executing one contract CALL from an indexed Zcash transaction.
///
/// Persisted by the indexer and exposed over RPC via `zalkanes_getExecution`.
/// Not part of the consensus state root; derived from canonical block data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Execution {
    pub txid: TxId,
    pub contract_id: ContractId,
    pub opcode: u16,
    pub success: bool,
    pub fuel_used: u64,
    /// Return data bytes (empty on failure).
    pub return_data: Vec<u8>,
    /// Trap reason, if any.
    pub error: Option<String>,
    pub state_root_before: StateRoot,
    pub state_root_after: StateRoot,
    pub block_height: BlockHeight,
    pub block_hash: BlockHash,
    /// V1 events (ADR-0008 §10): (emitting contract, event bytes), commit
    /// order. Always empty for v0 messages; defaulted so records persisted
    /// before V1 deserialize unchanged.
    #[serde(default)]
    pub events: Vec<(ContractId, Vec<u8>)>,
}

/// Zcash network.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Network {
    Mainnet,
    Testnet,
    Regtest,
}

impl Network {
    /// One-byte network identifier used in ContractId derivation.
    pub fn id_byte(self) -> u8 {
        match self {
            Network::Mainnet => 0x01,
            Network::Testnet => 0x02,
            Network::Regtest => 0x03,
        }
    }

    /// Human-readable network name as reported by Zebra's `getblockchaininfo`.
    pub fn zebra_name(self) -> &'static str {
        match self {
            Network::Mainnet => "main",
            Network::Testnet => "test",
            Network::Regtest => "regtest",
        }
    }
}

impl ContractId {
    /// Derive a ContractId per ADR 0004 / protocol-v0.md §8.
    ///
    /// ```text
    /// ContractId = BLAKE2b-256(
    ///     personalization = b"ZalkContractId0 ",
    ///     input = network_id || txid || output_index (BE u16) || code_hash
    /// )
    /// ```
    pub fn derive(network: Network, txid: &TxId, output_index: u16, code_hash: &CodeHash) -> Self {
        use crate::consensus::CONTRACT_ID_PERSONALIZATION;
        use blake2b_simd::Params;

        let mut input = Vec::with_capacity(1 + 32 + 2 + 32);
        input.push(network.id_byte());
        input.extend_from_slice(&txid.0);
        input.extend_from_slice(&output_index.to_be_bytes());
        input.extend_from_slice(&code_hash.0);

        let hash = Params::new()
            .hash_length(32)
            .personal(CONTRACT_ID_PERSONALIZATION)
            .hash(&input);

        let mut out = [0u8; 32];
        out.copy_from_slice(hash.as_bytes());
        ContractId(out)
    }

    pub fn as_hex(&self) -> String {
        hex::encode(self.0)
    }
}

impl CodeHash {
    /// Compute SHA-256 of WASM bytes.
    pub fn of(wasm: &[u8]) -> Self {
        use sha2::{Digest, Sha256};
        let digest = Sha256::digest(wasm);
        let mut out = [0u8; 32];
        out.copy_from_slice(&digest);
        CodeHash(out)
    }

    pub fn as_hex(&self) -> String {
        hex::encode(self.0)
    }
}

impl StateRoot {
    pub const ZERO: StateRoot = StateRoot([0u8; 32]);

    pub fn as_hex(&self) -> String {
        hex::encode(self.0)
    }
}

impl std::fmt::Display for ContractId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_hex())
    }
}

impl std::fmt::Display for StateRoot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_hex())
    }
}

impl std::fmt::Display for BlockHash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", hex::encode(self.0))
    }
}

impl std::fmt::Display for TxId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", hex::encode(self.0))
    }
}

// ── Protocol V1 derivations (ADR-0008) ──────────────────────────────────────

/// V1 external account id:
/// BLAKE2b-256("ZalkAccountId1  ", network_id ‖ compressed pubkey(33)).
/// `[0u8; 32]` is the reserved Anonymous account.
#[must_use]
pub fn v1_external_account_id(network: Network, compressed_pubkey: &[u8; 33]) -> [u8; 32] {
    use blake2b_simd::Params;
    let mut input = Vec::with_capacity(34);
    input.push(network.id_byte());
    input.extend_from_slice(compressed_pubkey);
    let hash = Params::new()
        .hash_length(32)
        .personal(crate::consensus::ACCOUNT_ID_PERSONALIZATION)
        .hash(&input);
    let mut out = [0u8; 32];
    out.copy_from_slice(hash.as_bytes());
    out
}

/// V1 spawned-contract id (ADR-0008 §9):
/// BLAKE2b-256("ZalkSpawnId1    ",
///             network_id ‖ txid(32) ‖ spawn_index(u16 BE) ‖ spawner(32) ‖ code_hash(32)).
#[must_use]
pub fn v1_spawned_contract_id(
    network: Network,
    txid: &TxId,
    spawn_index: u16,
    spawner: &ContractId,
    code_hash: &CodeHash,
) -> ContractId {
    use blake2b_simd::Params;
    let mut input = Vec::with_capacity(99);
    input.push(network.id_byte());
    input.extend_from_slice(&txid.0);
    input.extend_from_slice(&spawn_index.to_be_bytes());
    input.extend_from_slice(&spawner.0);
    input.extend_from_slice(&code_hash.0);
    let hash = Params::new()
        .hash_length(32)
        .personal(crate::consensus::SPAWN_ID_PERSONALIZATION)
        .hash(&input);
    let mut out = [0u8; 32];
    out.copy_from_slice(hash.as_bytes());
    ContractId(out)
}

/// V1 activation height for a network (`None` = V1 never active).
#[must_use]
pub fn v1_activation_height(network: Network) -> Option<u32> {
    match network {
        Network::Mainnet => crate::consensus::V1_MAINNET_ACTIVATION_HEIGHT,
        Network::Testnet => crate::consensus::V1_TESTNET_ACTIVATION_HEIGHT,
        Network::Regtest => crate::consensus::V1_REGTEST_ACTIVATION_HEIGHT,
    }
}

/// Is protocol V1 active at `height` on `network`?
#[must_use]
pub fn v1_active(network: Network, height: BlockHeight) -> bool {
    v1_activation_height(network).is_some_and(|h| height >= h)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn network_id_bytes_are_distinct() {
        assert_ne!(Network::Mainnet.id_byte(), Network::Testnet.id_byte());
        assert_ne!(Network::Testnet.id_byte(), Network::Regtest.id_byte());
        assert_ne!(Network::Mainnet.id_byte(), Network::Regtest.id_byte());
    }

    #[test]
    fn contract_id_is_deterministic() {
        let txid = TxId([1u8; 32]);
        let code_hash = CodeHash([2u8; 32]);
        let a = ContractId::derive(Network::Regtest, &txid, 1, &code_hash);
        let b = ContractId::derive(Network::Regtest, &txid, 1, &code_hash);
        assert_eq!(a, b);
    }

    #[test]
    fn contract_id_differs_by_network() {
        let txid = TxId([1u8; 32]);
        let code_hash = CodeHash([2u8; 32]);
        let main = ContractId::derive(Network::Mainnet, &txid, 0, &code_hash);
        let test = ContractId::derive(Network::Testnet, &txid, 0, &code_hash);
        let regtest = ContractId::derive(Network::Regtest, &txid, 0, &code_hash);
        assert_ne!(main, test);
        assert_ne!(test, regtest);
        assert_ne!(main, regtest);
    }

    #[test]
    fn contract_id_differs_by_output_index() {
        let txid = TxId([1u8; 32]);
        let code_hash = CodeHash([2u8; 32]);
        let a = ContractId::derive(Network::Regtest, &txid, 0, &code_hash);
        let b = ContractId::derive(Network::Regtest, &txid, 1, &code_hash);
        assert_ne!(a, b);
    }

    #[test]
    fn code_hash_of_empty() {
        // SHA-256("") = e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855
        let h = CodeHash::of(b"");
        assert_eq!(
            h.as_hex(),
            "e3b0c44298fc1c149afbf4c8996fb924\
             27ae41e4649b934ca495991b7852b855"
        );
    }
}
