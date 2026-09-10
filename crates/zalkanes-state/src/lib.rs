//! # zalkanes-state
//!
//! RocksDB-backed state engine with deterministic state root.
//!
//! See `docs/adr/0006-state-root.md`.

#![forbid(unsafe_code)]

use blake2b_simd::Params;
use zalkanes_core::{
    consensus::{STATE_LEAF_PERSONALIZATION, STATE_ROOT_PERSONALIZATION},
    types::{BlockHash, BlockHeight, CodeHash, ContractId, StateRoot},
};

/// All state required to reproduce a state root.
/// Used by the testkit; production uses RocksDB.
#[derive(Debug, Default)]
pub struct MemoryState {
    /// contract_id → (code_hash, code_bytes)
    pub contracts: std::collections::BTreeMap<[u8; 32], (CodeHash, Vec<u8>)>,
    /// (contract_id, key) → value
    pub storage: std::collections::BTreeMap<([u8; 32], Vec<u8>), Vec<u8>>,
}

impl MemoryState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn deploy(&mut self, id: ContractId, code_hash: CodeHash, wasm: Vec<u8>) {
        self.contracts.insert(id.0, (code_hash, wasm));
    }

    pub fn storage_get(&self, contract: &ContractId, key: &[u8]) -> Option<Vec<u8>> {
        self.storage.get(&(contract.0, key.to_vec())).cloned()
    }

    pub fn storage_set(&mut self, contract: ContractId, key: Vec<u8>, value: Vec<u8>) {
        self.storage.insert((contract.0, key), value);
    }

    pub fn storage_delete(&mut self, contract: &ContractId, key: &[u8]) {
        self.storage.remove(&(contract.0, key.to_vec()));
    }

    pub fn has_contract(&self, id: &ContractId) -> bool {
        self.contracts.contains_key(&id.0)
    }

    /// Compute the state root over all storage entries.
    ///
    /// Algorithm (ADR 0006):
    /// 1. For each (contract_id, key, value): compute a leaf hash.
    /// 2. Sort leaf hashes lexicographically.
    /// 3. Hash the sorted concatenation.
    pub fn compute_root(&self) -> StateRoot {
        let mut leaves: Vec<[u8; 32]> = self
            .storage
            .iter()
            .map(|((cid, key), value)| compute_leaf(cid, key, value))
            .collect();

        leaves.sort_unstable();

        let mut input = Vec::with_capacity(leaves.len() * 32);
        for leaf in &leaves {
            input.extend_from_slice(leaf);
        }

        let hash = Params::new()
            .hash_length(32)
            .personal(STATE_ROOT_PERSONALIZATION)
            .hash(&input);

        let mut out = [0u8; 32];
        out.copy_from_slice(hash.as_bytes());
        StateRoot(out)
    }
}

fn compute_leaf(contract_id: &[u8; 32], key: &[u8], value: &[u8]) -> [u8; 32] {
    let key_len = key.len() as u16;
    let val_len = value.len() as u32;

    let mut input = Vec::with_capacity(32 + 2 + key.len() + 4 + value.len());
    input.extend_from_slice(contract_id);
    input.extend_from_slice(&key_len.to_be_bytes());
    input.extend_from_slice(key);
    input.extend_from_slice(&val_len.to_be_bytes());
    input.extend_from_slice(value);

    let hash = Params::new()
        .hash_length(32)
        .personal(STATE_LEAF_PERSONALIZATION)
        .hash(&input);

    let mut out = [0u8; 32];
    out.copy_from_slice(hash.as_bytes());
    out
}

/// Per-height commit record for reorg support.
#[derive(Debug, Clone)]
pub struct HeightRecord {
    pub height: BlockHeight,
    pub zcash_block_hash: BlockHash,
    pub state_root: StateRoot,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_state_root_is_deterministic() {
        let a = MemoryState::new().compute_root();
        let b = MemoryState::new().compute_root();
        assert_eq!(a, b);
    }

    #[test]
    fn root_changes_on_write() {
        let mut state = MemoryState::new();
        let r0 = state.compute_root();
        let cid = ContractId([1u8; 32]);
        state.storage_set(cid, b"key".to_vec(), b"value".to_vec());
        let r1 = state.compute_root();
        assert_ne!(r0, r1);
    }

    #[test]
    fn root_is_order_independent() {
        let cid = ContractId([1u8; 32]);
        let mut a = MemoryState::new();
        a.storage_set(cid, b"k1".to_vec(), b"v1".to_vec());
        a.storage_set(cid, b"k2".to_vec(), b"v2".to_vec());

        let mut b = MemoryState::new();
        b.storage_set(cid, b"k2".to_vec(), b"v2".to_vec());
        b.storage_set(cid, b"k1".to_vec(), b"v1".to_vec());

        assert_eq!(a.compute_root(), b.compute_root());
    }

    #[test]
    fn delete_restores_root() {
        let cid = ContractId([1u8; 32]);
        let mut state = MemoryState::new();
        let r0 = state.compute_root();
        state.storage_set(cid, b"key".to_vec(), b"val".to_vec());
        state.storage_delete(&cid, b"key");
        let r1 = state.compute_root();
        assert_eq!(r0, r1);
    }
}
