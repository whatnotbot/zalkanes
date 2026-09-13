//! Protocol V1 block execution support (ADR-0008).
//!
//! Maintains the in-block overlay that gives V1 messages sequential
//! intra-block visibility, resolves V1 CALL payloads (carriers + auth),
//! runs them through `zalkanes_runtime::v1`, and rebuilds the final
//! `BlockCommit` when a block contained at least one V1 message.
//!
//! v0-only blocks never touch this path's commit rebuild: their commit is
//! byte-identical to the frozen v0 pipeline.

use std::collections::BTreeMap;

use zalkanes_core::types::{CodeHash, ContractId};
use zalkanes_runtime::v1::{AssetBytes, HolderBytes, V1StateView};
use zalkanes_state::{BlockCommit, StateStore};

/// Per-block V1 bookkeeping.
pub(crate) struct BlockV1State {
    /// V1 active at this height (network activation reached)?
    pub active: bool,
    /// True once any V1 message EXECUTED in this block (the commit is then
    /// rebuilt from the overlay — ADR-0008 §11 mixed-block rule).
    pub any_v1: bool,
    /// Final storage overlay: every write of the block in message order.
    pub storage: BTreeMap<(ContractId, Vec<u8>), Option<Vec<u8>>>,
    /// Final ledger overlay.
    pub ledger: BTreeMap<(HolderBytes, AssetBytes), u128>,
    /// Contracts spawned in this block, in spawn order.
    pub spawned: Vec<(ContractId, CodeHash, Vec<u8>)>,
}

impl BlockV1State {
    pub fn new(active: bool) -> Self {
        Self {
            active,
            any_v1: false,
            storage: BTreeMap::new(),
            ledger: BTreeMap::new(),
            spawned: Vec::new(),
        }
    }

    /// Mirror a v0 storage write into the overlay (only meaningful while
    /// V1 is active; harmless otherwise).
    pub fn mirror_storage(&mut self, contract: ContractId, key: Vec<u8>, value: Option<Vec<u8>>) {
        if self.active {
            self.storage.insert((contract, key), value);
        }
    }

    /// Rebuild the commit's storage/ledger contents from the overlay
    /// (called only when `any_v1`). Deploys keep v0 ordering; spawned
    /// contracts are appended as deploys in spawn order.
    pub fn rebuild_commit(&self, commit: &mut BlockCommit) {
        commit.upserts.clear();
        commit.deletes.clear();
        for ((contract, key), value) in &self.storage {
            match value {
                Some(v) => commit.upserts.push((*contract, key.clone(), v.clone())),
                None => commit.deletes.push((*contract, key.clone())),
            }
        }
        commit.ledger_upserts.clear();
        commit.ledger_deletes.clear();
        for ((holder, asset), amount) in &self.ledger {
            if *amount > 0 {
                commit.ledger_upserts.push((*holder, *asset, *amount));
            } else {
                commit.ledger_deletes.push((*holder, *asset));
            }
        }
        for (id, code_hash, code) in &self.spawned {
            commit.deploys.push((*id, *code_hash, code.clone()));
        }
    }
}

/// Read view for V1 execution: block overlay first, then pending deploys,
/// then the pre-block store.
pub(crate) struct OverlayView<'a> {
    pub store: &'a dyn StateStore,
    pub commit: &'a BlockCommit,
    pub v1: &'a BlockV1State,
}

impl OverlayView<'_> {
    pub fn find_code_by_hash(&self, hash: &CodeHash) -> Option<Vec<u8>> {
        for (_, spawned_hash, code) in &self.v1.spawned {
            if spawned_hash == hash {
                return Some(code.clone());
            }
        }
        for (_, deploy_hash, code) in &self.commit.deploys {
            if deploy_hash == hash {
                return Some(code.clone());
            }
        }
        let mut found = None;
        self.store.for_each_contract(&mut |_, code_hash, wasm| {
            if found.is_none() && code_hash == hash {
                found = Some(wasm.to_vec());
            }
        });
        found
    }
}

impl V1StateView for OverlayView<'_> {
    fn contract_code(&self, id: &ContractId) -> Option<(CodeHash, Vec<u8>)> {
        for (spawned_id, hash, code) in &self.v1.spawned {
            if spawned_id == id {
                return Some((*hash, code.clone()));
            }
        }
        for (deploy_id, hash, code) in &self.commit.deploys {
            if deploy_id == id {
                return Some((*hash, code.clone()));
            }
        }
        self.store.get_contract(id)
    }

    fn code_by_hash(&self, hash: &CodeHash) -> Option<Vec<u8>> {
        self.find_code_by_hash(hash)
    }

    fn storage_get(&self, contract: &ContractId, key: &[u8]) -> Option<Vec<u8>> {
        if let Some(entry) = self.v1.storage.get(&(*contract, key.to_vec())) {
            return entry.clone();
        }
        // v0 writes of this block are mirrored into the overlay whenever V1
        // is active, so falling through here means the key is untouched in
        // this block.
        self.store.storage_get(contract, key)
    }

    fn ledger_get(&self, holder: &HolderBytes, asset: &AssetBytes) -> Option<u128> {
        if let Some(amount) = self.v1.ledger.get(&(*holder, *asset)) {
            return Some(*amount);
        }
        self.store.ledger_get(holder, asset)
    }
}
