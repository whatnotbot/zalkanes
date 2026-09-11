//! # zalkanes-state
//!
//! Persistent state engine with deterministic state root.
//!
//! Two backends implement [`StateStore`]:
//! - [`MemoryState`] — in-memory (testkit, unit tests)
//! - [`RocksState`] — RocksDB (production), persisted on disk
//!
//! The state root is BLAKE2b-256 over the sorted leaves of BOTH deployed
//! contracts and storage entries, so deploying a contract changes the root
//! even before any storage is written.
//!
//! `commit_block` applies a block's writes atomically and records an undo
//! journal so `rollback_to` can reverse a reorg. The persisted `m:r` metadata
//! root is advisory; `compute_root()` over the DB is always authoritative.
//!
//! See `docs/adr/0006-state-root.md`.

#![forbid(unsafe_code)]

use anyhow::{Context, Result};
use blake2b_simd::Params;
use zalkanes_core::{
    consensus::{STATE_LEAF_PERSONALIZATION, STATE_ROOT_PERSONALIZATION},
    types::{BlockHash, BlockHeight, CodeHash, ContractId, Execution, StateRoot, TxId},
};

// ── BlockCommit / undo journal ───────────────────────────────────────────────

/// All writes a single Zcash block produced, committed atomically.
#[derive(Debug, Clone)]
pub struct BlockCommit {
    pub height: BlockHeight,
    pub zcash_block_hash: BlockHash,
    /// New contracts deployed in this block.
    pub deploys: Vec<(ContractId, CodeHash, Vec<u8>)>,
    /// Storage upserts: (contract_id, key, value).
    pub upserts: Vec<(ContractId, Vec<u8>, Vec<u8>)>,
    /// Storage deletions: (contract_id, key).
    pub deletes: Vec<(ContractId, Vec<u8>)>,
}

/// A single storage mutation recorded for undo.
#[derive(Debug, Clone)]
enum UndoOp {
    /// Key was written; store the previous value (None = did not exist).
    Set {
        contract: ContractId,
        key: Vec<u8>,
        prev: Option<Vec<u8>>,
    },
    /// Key was deleted; store the previous value.
    Delete {
        contract: ContractId,
        key: Vec<u8>,
        prev: Vec<u8>,
    },
}

// ── StateStore trait ─────────────────────────────────────────────────────────

/// Read/write contract state with deterministic root, metadata, and rollback.
///
/// Callback: visit a deployed contract.
pub type ContractVisitor<'a> = dyn FnMut(&ContractId, &CodeHash, &[u8]) + 'a;
/// Callback: visit a storage entry.
pub type StorageVisitor<'a> = dyn FnMut(&ContractId, &[u8], &[u8]) + 'a;

/// Object-safe so it can be shared as `Box<dyn StateStore>` behind a lock
/// between the indexer (writer) and RPC server (reader).
pub trait StateStore: Send + Sync {
    // Contracts (read)
    fn has_contract(&self, id: &ContractId) -> bool;
    fn get_contract(&self, id: &ContractId) -> Option<(CodeHash, Vec<u8>)>;

    // Storage (read)
    fn storage_get(&self, contract: &ContractId, key: &[u8]) -> Option<Vec<u8>>;

    // State root (always authoritative — recomputed from the store)
    fn compute_root(&self) -> StateRoot;

    // Iteration
    fn for_each_contract(&self, f: &mut ContractVisitor<'_>);
    fn for_each_storage(&self, f: &mut StorageVisitor<'_>);

    // Indexer metadata
    fn indexed_height(&self) -> Option<BlockHeight>;
    fn indexed_block_hash(&self) -> Option<BlockHash>;
    /// Advisory persisted root; `compute_root()` is authoritative.
    fn persisted_state_root(&self) -> Option<StateRoot>;
    fn height_history(&self) -> Vec<HeightRecord>;

    // Writes (require &mut)
    fn commit_block(&mut self, commit: BlockCommit) -> Result<StateRoot>;
    fn rollback_to(&mut self, target_height: BlockHeight) -> Result<()>;

    // Execution records (derived, not consensus root)
    fn save_executions(&mut self, executions: &[Execution]) -> Result<()>;
    fn execution(&self, txid: &TxId) -> Option<Execution>;
    fn executions_at_height(&self, height: BlockHeight) -> Vec<Execution>;
}

/// Per-height commit record for reorg support.
#[derive(Debug, Clone)]
pub struct HeightRecord {
    pub height: BlockHeight,
    pub zcash_block_hash: BlockHash,
    pub state_root: StateRoot,
}

// ── Leaf hashing (shared, consensus-critical) ────────────────────────────────

fn contract_leaf(contract_id: &[u8; 32], code_hash: &[u8; 32], wasm: &[u8]) -> [u8; 32] {
    let code_len = wasm.len() as u32;
    let mut input = Vec::with_capacity(32 + 32 + 4 + wasm.len());
    input.extend_from_slice(contract_id);
    input.extend_from_slice(code_hash);
    input.extend_from_slice(&code_len.to_be_bytes());
    input.extend_from_slice(wasm);

    let hash = Params::new()
        .hash_length(32)
        .personal(STATE_LEAF_PERSONALIZATION)
        .hash(&input);
    let mut out = [0u8; 32];
    out.copy_from_slice(hash.as_bytes());
    out
}

fn storage_leaf(contract_id: &[u8; 32], key: &[u8], value: &[u8]) -> [u8; 32] {
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

fn root_from_leaves(mut leaves: Vec<[u8; 32]>) -> StateRoot {
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

fn collect_leaves<S: StateStore>(store: &S) -> Vec<[u8; 32]> {
    let mut leaves = Vec::new();
    store.for_each_contract(&mut |cid, code_hash, wasm| {
        leaves.push(contract_leaf(&cid.0, &code_hash.0, wasm));
    });
    store.for_each_storage(&mut |cid, key, value| {
        leaves.push(storage_leaf(&cid.0, key, value));
    });
    leaves
}

/// Compute the state root the store would have after applying `commit`,
/// without mutating the store. Used for execution records (before/after roots)
/// and by the indexer for the authoritative pre-commit root.
pub fn projected_root(store: &dyn StateStore, commit: &BlockCommit) -> StateRoot {
    // Build a working view: start from existing leaves, then apply the commit.
    // Contracts are keyed by contract_id; storage by (contract_id, key).
    let mut contract_wasm: std::collections::BTreeMap<[u8; 32], ([u8; 32], Vec<u8>)> =
        std::collections::BTreeMap::new();
    let mut storage_map: std::collections::BTreeMap<([u8; 32], Vec<u8>), Vec<u8>> =
        std::collections::BTreeMap::new();

    store.for_each_contract(&mut |cid, code_hash, wasm| {
        contract_wasm.insert(cid.0, (code_hash.0, wasm.to_vec()));
    });
    store.for_each_storage(&mut |cid, key, value| {
        storage_map.insert((cid.0, key.to_vec()), value.to_vec());
    });

    for (id, code_hash, wasm) in &commit.deploys {
        contract_wasm.insert(id.0, (code_hash.0, wasm.clone()));
    }
    for (cid, key, value) in &commit.upserts {
        storage_map.insert((cid.0, key.clone()), value.clone());
    }
    for (cid, key) in &commit.deletes {
        storage_map.remove(&(cid.0, key.clone()));
    }

    let mut leaves = Vec::new();
    for (id, (code_hash, wasm)) in &contract_wasm {
        leaves.push(contract_leaf(id, code_hash, wasm));
    }
    for ((id, key), value) in &storage_map {
        leaves.push(storage_leaf(id, key, value));
    }
    root_from_leaves(leaves)
}

// ── MemoryState ──────────────────────────────────────────────────────────────

/// In-memory state backend. Used by the testkit and unit tests.
#[derive(Debug, Clone)]
pub struct MemoryState {
    pub contracts: std::collections::BTreeMap<[u8; 32], (CodeHash, Vec<u8>)>,
    pub storage: std::collections::BTreeMap<([u8; 32], Vec<u8>), Vec<u8>>,
    metadata: Metadata,
    history: Vec<HeightRecord>,
    undo: std::collections::BTreeMap<BlockHeight, Vec<UndoOp>>,
    executions: std::collections::BTreeMap<TxId, Execution>,
}

#[derive(Debug, Clone, Default)]
struct Metadata {
    indexed_height: Option<BlockHeight>,
    indexed_block_hash: Option<BlockHash>,
    persisted_root: Option<StateRoot>,
}

impl Default for MemoryState {
    fn default() -> Self {
        Self::new()
    }
}

impl MemoryState {
    pub fn new() -> Self {
        Self {
            contracts: Default::default(),
            storage: Default::default(),
            metadata: Default::default(),
            history: Vec::new(),
            undo: Default::default(),
            executions: Default::default(),
        }
    }
}

impl StateStore for MemoryState {
    fn has_contract(&self, id: &ContractId) -> bool {
        self.contracts.contains_key(&id.0)
    }
    fn get_contract(&self, id: &ContractId) -> Option<(CodeHash, Vec<u8>)> {
        self.contracts.get(&id.0).cloned()
    }
    fn storage_get(&self, contract: &ContractId, key: &[u8]) -> Option<Vec<u8>> {
        self.storage.get(&(contract.0, key.to_vec())).cloned()
    }
    fn compute_root(&self) -> StateRoot {
        root_from_leaves(collect_leaves(self))
    }
    fn for_each_contract(&self, f: &mut ContractVisitor<'_>) {
        for (id, (code_hash, wasm)) in &self.contracts {
            let cid = ContractId(*id);
            f(&cid, code_hash, wasm);
        }
    }
    fn for_each_storage(&self, f: &mut StorageVisitor<'_>) {
        for ((id, key), value) in &self.storage {
            let cid = ContractId(*id);
            f(&cid, key, value);
        }
    }
    fn indexed_height(&self) -> Option<BlockHeight> {
        self.metadata.indexed_height
    }
    fn indexed_block_hash(&self) -> Option<BlockHash> {
        self.metadata.indexed_block_hash
    }
    fn persisted_state_root(&self) -> Option<StateRoot> {
        self.metadata.persisted_root
    }
    fn height_history(&self) -> Vec<HeightRecord> {
        self.history.clone()
    }

    fn commit_block(&mut self, commit: BlockCommit) -> Result<StateRoot> {
        let mut undo = Vec::new();

        for (id, code_hash, wasm) in &commit.deploys {
            let prev = self.contracts.insert(id.0, (*code_hash, wasm.clone()));
            // Record deploy for undo (contract was not present, or replaced).
            // For simplicity, deployments are keyed so rollback re-deletes them.
            let _ = prev;
        }
        for (cid, key, value) in &commit.upserts {
            let prev = self.storage.insert((cid.0, key.clone()), value.clone());
            undo.push(UndoOp::Set {
                contract: ContractId(cid.0),
                key: key.clone(),
                prev,
            });
        }
        for (cid, key) in &commit.deletes {
            let prev = self.storage.remove(&(cid.0, key.clone()));
            if let Some(prev) = prev {
                undo.push(UndoOp::Delete {
                    contract: ContractId(cid.0),
                    key: key.clone(),
                    prev,
                });
            }
        }

        // Record deployments for undo.
        for (id, _, _) in &commit.deploys {
            undo.push(UndoOp::Set {
                contract: ContractId(id.0),
                key: Vec::new(),
                prev: None,
            });
        }

        let root = self.compute_root();
        self.metadata.indexed_height = Some(commit.height);
        self.metadata.indexed_block_hash = Some(commit.zcash_block_hash);
        self.metadata.persisted_root = Some(root);
        self.history.push(HeightRecord {
            height: commit.height,
            zcash_block_hash: commit.zcash_block_hash,
            state_root: root,
        });
        self.undo.insert(commit.height, undo);
        Ok(root)
    }

    fn rollback_to(&mut self, target_height: BlockHeight) -> Result<()> {
        // Collect heights above target, descending.
        let mut heights: Vec<BlockHeight> = self
            .undo
            .keys()
            .copied()
            .filter(|h| *h > target_height)
            .collect();
        heights.sort_unstable_by(|a, b| b.cmp(a));

        for h in heights {
            if let Some(ops) = self.undo.remove(&h) {
                for op in ops.into_iter().rev() {
                    match op {
                        UndoOp::Set {
                            contract,
                            key,
                            prev,
                        } => {
                            if key.is_empty() {
                                // Deployment undo: remove contract.
                                self.contracts.remove(&contract.0);
                            } else {
                                match prev {
                                    Some(v) => {
                                        self.storage.insert((contract.0, key), v);
                                    }
                                    None => {
                                        self.storage.remove(&(contract.0, key));
                                    }
                                }
                            }
                        }
                        UndoOp::Delete {
                            contract,
                            key,
                            prev,
                        } => {
                            self.storage.insert((contract.0, key), prev);
                        }
                    }
                }
            }
            self.history.retain(|r| r.height <= target_height);
        }

        // Restore metadata to the target height.
        if let Some(rec) = self.history.last() {
            self.metadata.indexed_height = Some(rec.height);
            self.metadata.indexed_block_hash = Some(rec.zcash_block_hash);
            self.metadata.persisted_root = Some(rec.state_root);
        } else {
            self.metadata = Metadata::default();
        }
        Ok(())
    }

    fn save_executions(&mut self, executions: &[Execution]) -> Result<()> {
        for e in executions {
            self.executions.insert(e.txid, e.clone());
        }
        Ok(())
    }

    fn execution(&self, txid: &TxId) -> Option<Execution> {
        self.executions.get(txid).cloned()
    }

    fn executions_at_height(&self, height: BlockHeight) -> Vec<Execution> {
        self.executions
            .values()
            .filter(|e| e.block_height == height)
            .cloned()
            .collect()
    }
}

// ── RocksState ───────────────────────────────────────────────────────────────

fn storage_db_key(contract: &ContractId, key: &[u8]) -> Vec<u8> {
    let mut k = Vec::with_capacity(1 + 32 + key.len());
    k.push(b's');
    k.extend_from_slice(&contract.0);
    k.extend_from_slice(key);
    k
}

fn contract_db_key(contract: &ContractId) -> Vec<u8> {
    let mut k = Vec::with_capacity(1 + 32);
    k.push(b'c');
    k.extend_from_slice(&contract.0);
    k
}

fn height_db_key(height: BlockHeight) -> [u8; 5] {
    let mut k = [0u8; 5];
    k[0] = b'h';
    k[1..].copy_from_slice(&height.to_be_bytes());
    k
}

fn undo_db_key(height: BlockHeight) -> [u8; 5] {
    let mut k = [0u8; 5];
    k[0] = b'u';
    k[1..].copy_from_slice(&height.to_be_bytes());
    k
}

fn exec_db_key(txid: &TxId) -> Vec<u8> {
    let mut k = Vec::with_capacity(1 + 32);
    k.push(b'e');
    k.extend_from_slice(&txid.0);
    k
}

fn exec_height_db_key(height: BlockHeight) -> [u8; 5] {
    let mut k = [0u8; 5];
    k[0] = b'x';
    k[1..].copy_from_slice(&height.to_be_bytes());
    k
}

/// Serialize an undo op list (internal, not consensus data).
fn encode_undo(ops: &[UndoOp]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&(ops.len() as u32).to_be_bytes());
    for op in ops {
        match op {
            UndoOp::Set {
                contract,
                key,
                prev,
            } => {
                out.push(0x01);
                out.extend_from_slice(&contract.0);
                out.extend_from_slice(&(key.len() as u16).to_be_bytes());
                out.extend_from_slice(key);
                match prev {
                    Some(p) => {
                        out.push(0x01);
                        out.extend_from_slice(&(p.len() as u32).to_be_bytes());
                        out.extend_from_slice(p);
                    }
                    None => out.push(0x00),
                }
            }
            UndoOp::Delete {
                contract,
                key,
                prev,
            } => {
                out.push(0x02);
                out.extend_from_slice(&contract.0);
                out.extend_from_slice(&(key.len() as u16).to_be_bytes());
                out.extend_from_slice(key);
                out.extend_from_slice(&(prev.len() as u32).to_be_bytes());
                out.extend_from_slice(prev);
            }
        }
    }
    out
}

fn decode_undo(mut bytes: &[u8]) -> Vec<UndoOp> {
    let mut ops = Vec::new();
    if bytes.len() < 4 {
        return ops;
    }
    let n = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;
    bytes = &bytes[4..];
    for _ in 0..n {
        let Some(&tag) = bytes.first() else { break };
        bytes = &bytes[1..];
        if bytes.len() < 32 {
            break;
        }
        let mut cid = [0u8; 32];
        cid.copy_from_slice(&bytes[0..32]);
        bytes = &bytes[32..];
        if bytes.len() < 2 {
            break;
        }
        let key_len = u16::from_be_bytes([bytes[0], bytes[1]]) as usize;
        bytes = &bytes[2..];
        if bytes.len() < key_len {
            break;
        }
        let key = bytes[..key_len].to_vec();
        bytes = &bytes[key_len..];

        match tag {
            0x01 => {
                // Set
                let Some(&has_prev) = bytes.first() else {
                    break;
                };
                bytes = &bytes[1..];
                let prev = if has_prev == 0x01 {
                    if bytes.len() < 4 {
                        break;
                    }
                    let plen =
                        u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;
                    bytes = &bytes[4..];
                    if bytes.len() < plen {
                        break;
                    }
                    let p = bytes[..plen].to_vec();
                    bytes = &bytes[plen..];
                    Some(p)
                } else {
                    None
                };
                ops.push(UndoOp::Set {
                    contract: ContractId(cid),
                    key,
                    prev,
                });
            }
            0x02 => {
                // Delete
                if bytes.len() < 4 {
                    break;
                }
                let plen = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;
                bytes = &bytes[4..];
                if bytes.len() < plen {
                    break;
                }
                let prev = bytes[..plen].to_vec();
                bytes = &bytes[plen..];
                ops.push(UndoOp::Delete {
                    contract: ContractId(cid),
                    key,
                    prev,
                });
            }
            _ => break,
        }
    }
    ops
}

/// A persistent RocksDB-backed state store.
///
/// Keyspaces (single default column family, prefixed):
/// - `m:h` → indexed height (u32 BE)
/// - `m:b` → indexed Zcash block hash (32 bytes)
/// - `m:r` → advisory state root (32 bytes)
/// - `c:<contract_id>` → code_hash(32) || wasm
/// - `s:<contract_id><key>` → value
/// - `h:<height>` → zcash_block_hash(32) || state_root(32)
/// - `u:<height>` → undo journal (see encode/decode_undo)
pub struct RocksState {
    db: rocksdb::DB,
}

impl RocksState {
    pub fn open(path: &std::path::Path) -> Result<Self> {
        let mut opts = rocksdb::Options::default();
        opts.create_if_missing(true);
        opts.set_keep_log_file_num(4);

        // During a rolling deploy the previous container may still hold the
        // RocksDB lock briefly. Retry with backoff rather than crashing.
        for _ in 0..60 {
            match rocksdb::DB::open(&opts, path) {
                Ok(db) => return Ok(Self { db }),
                Err(e) => {
                    let s = e.to_string();
                    if s.contains("Resource temporarily unavailable") || s.contains("lock") {
                        std::thread::sleep(std::time::Duration::from_millis(500));
                        continue;
                    }
                    return Err(e)
                        .with_context(|| format!("failed to open RocksDB at {}", path.display()));
                }
            }
        }
        Err(anyhow::anyhow!(
            "failed to open RocksDB at {} (lock held too long)",
            path.display()
        ))
    }

    pub fn raw_get(&self, key: &[u8]) -> Option<Vec<u8>> {
        self.db.get(key).ok().flatten()
    }

    pub fn raw_put(&self, key: &[u8], value: &[u8]) -> Result<()> {
        self.db.put(key, value).context("rocksdb put failed")
    }
}

impl StateStore for RocksState {
    fn has_contract(&self, id: &ContractId) -> bool {
        self.db.get(contract_db_key(id)).ok().flatten().is_some()
    }
    fn get_contract(&self, id: &ContractId) -> Option<(CodeHash, Vec<u8>)> {
        let v = self.db.get(contract_db_key(id)).ok().flatten()?;
        if v.len() < 32 {
            return None;
        }
        let mut code_hash = [0u8; 32];
        code_hash.copy_from_slice(&v[0..32]);
        Some((CodeHash(code_hash), v[32..].to_vec()))
    }
    fn storage_get(&self, contract: &ContractId, key: &[u8]) -> Option<Vec<u8>> {
        self.db.get(storage_db_key(contract, key)).ok().flatten()
    }
    fn compute_root(&self) -> StateRoot {
        root_from_leaves(collect_leaves(self))
    }
    fn for_each_contract(&self, f: &mut ContractVisitor<'_>) {
        let iter = self.db.iterator(rocksdb::IteratorMode::From(
            b"c",
            rocksdb::Direction::Forward,
        ));
        for item in iter.flatten() {
            let (key, value) = item;
            if key.is_empty() || key[0] != b'c' || key.len() != 33 || value.len() < 32 {
                continue;
            }
            let mut cid = [0u8; 32];
            cid.copy_from_slice(&key[1..33]);
            let mut code_hash = [0u8; 32];
            code_hash.copy_from_slice(&value[0..32]);
            let contract = ContractId(cid);
            let ch = CodeHash(code_hash);
            f(&contract, &ch, &value[32..]);
        }
    }
    fn for_each_storage(&self, f: &mut StorageVisitor<'_>) {
        let iter = self.db.iterator(rocksdb::IteratorMode::From(
            b"s",
            rocksdb::Direction::Forward,
        ));
        for item in iter.flatten() {
            let (key, value) = item;
            if key.is_empty() || key[0] != b's' || key.len() < 33 {
                continue;
            }
            let mut cid = [0u8; 32];
            cid.copy_from_slice(&key[1..33]);
            let contract = ContractId(cid);
            f(&contract, &key[33..], &value);
        }
    }
    fn indexed_height(&self) -> Option<BlockHeight> {
        self.db
            .get(b"m:h")
            .ok()
            .flatten()
            .and_then(|v| v.as_slice().try_into().ok().map(u32::from_be_bytes))
    }
    fn indexed_block_hash(&self) -> Option<BlockHash> {
        self.db
            .get(b"m:b")
            .ok()
            .flatten()
            .and_then(|v| v.as_slice().try_into().ok().map(BlockHash))
    }
    fn persisted_state_root(&self) -> Option<StateRoot> {
        self.db
            .get(b"m:r")
            .ok()
            .flatten()
            .and_then(|v| v.as_slice().try_into().ok().map(StateRoot))
    }
    fn height_history(&self) -> Vec<HeightRecord> {
        let mut out = Vec::new();
        let iter = self.db.iterator(rocksdb::IteratorMode::From(
            b"h",
            rocksdb::Direction::Forward,
        ));
        for item in iter.flatten() {
            let (key, value) = item;
            if key.len() != 5 || key[0] != b'h' || value.len() != 64 {
                continue;
            }
            let height = u32::from_be_bytes([key[1], key[2], key[3], key[4]]);
            let mut hash = [0u8; 32];
            hash.copy_from_slice(&value[0..32]);
            let mut root = [0u8; 32];
            root.copy_from_slice(&value[32..64]);
            out.push(HeightRecord {
                height,
                zcash_block_hash: BlockHash(hash),
                state_root: StateRoot(root),
            });
        }
        out
    }

    fn commit_block(&mut self, commit: BlockCommit) -> Result<StateRoot> {
        let mut batch = rocksdb::WriteBatch::default();
        let mut undo = Vec::new();

        for (id, code_hash, wasm) in &commit.deploys {
            let key = contract_db_key(id);
            let mut v = Vec::with_capacity(32 + wasm.len());
            v.extend_from_slice(&code_hash.0);
            v.extend_from_slice(wasm);
            // Record whether the contract already existed (for undo).
            let prev = self.db.get(&key).ok().flatten();
            let _ = prev;
            batch.put(key, v);
            undo.push(UndoOp::Set {
                contract: *id,
                key: Vec::new(),
                prev: None,
            });
        }
        for (cid, key, value) in &commit.upserts {
            let db_key = storage_db_key(cid, key);
            let prev = self.db.get(&db_key).ok().flatten();
            batch.put(db_key, value);
            undo.push(UndoOp::Set {
                contract: *cid,
                key: key.clone(),
                prev,
            });
        }
        // Deletes see the commit's own upserts (a key upserted and deleted in
        // the SAME commit ends up deleted), mirroring MemoryState and
        // `projected_root` exactly. The undo value is the value effectively
        // removed at this point (the same-commit upsert if present, else the
        // pre-commit value), so reversed-order rollback restores the exact
        // pre-commit state.
        let upserted: std::collections::HashMap<([u8; 32], Vec<u8>), Vec<u8>> = commit
            .upserts
            .iter()
            .map(|(cid, key, value)| ((cid.0, key.clone()), value.clone()))
            .collect();
        for (cid, key) in &commit.deletes {
            let db_key = storage_db_key(cid, key);
            let effective = upserted
                .get(&(cid.0, key.clone()))
                .cloned()
                .or_else(|| self.db.get(&db_key).ok().flatten());
            if let Some(prev) = effective {
                batch.delete(db_key);
                undo.push(UndoOp::Delete {
                    contract: *cid,
                    key: key.clone(),
                    prev,
                });
            }
        }

        // Write the journal BEFORE applying, so a crash mid-write is recoverable
        // by replaying from the journal on startup (see indexer reorg logic).
        batch.put(undo_db_key(commit.height), encode_undo(&undo));

        // Metadata + height record.
        batch.put(b"m:h", commit.height.to_be_bytes());
        batch.put(b"m:b", commit.zcash_block_hash.0);

        // Compute the post-commit root BEFORE writing, so the data, undo
        // journal, metadata, height record, AND the root all land in ONE
        // atomic WriteBatch. (Previously the height record carried a
        // placeholder root patched by two follow-up writes, leaving a crash
        // window with a zeroed persisted root.)
        let root = projected_root(self, &commit);
        let hkey = height_db_key(commit.height);
        let mut rec = Vec::with_capacity(64);
        rec.extend_from_slice(&commit.zcash_block_hash.0);
        rec.extend_from_slice(&root.0);
        batch.put(hkey, rec);
        batch.put(b"m:r", root.0);

        self.db.write(batch).context("rocksdb write batch failed")?;

        // Defensive cross-check: the applied state must hash to the projected
        // root. A mismatch is a consensus-critical bug and must be loud.
        let applied = self.compute_root();
        if applied != root {
            anyhow::bail!(
                "post-commit state root mismatch at height {}: projected {} != applied {}",
                commit.height,
                root.0
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect::<String>(),
                applied
                    .0
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect::<String>()
            );
        }

        Ok(root)
    }

    fn rollback_to(&mut self, target_height: BlockHeight) -> Result<()> {
        // Heights above target, descending.
        let mut heights: Vec<BlockHeight> = self
            .height_history()
            .into_iter()
            .map(|r| r.height)
            .filter(|h| *h > target_height)
            .collect();
        heights.sort_unstable_by(|a, b| b.cmp(a));

        for h in heights {
            if let Some(ops) = self.raw_get(&undo_db_key(h)) {
                let ops = decode_undo(&ops);
                let mut batch = rocksdb::WriteBatch::default();
                for op in ops.into_iter().rev() {
                    match op {
                        UndoOp::Set {
                            contract,
                            key,
                            prev,
                        } => {
                            if key.is_empty() {
                                batch.delete(contract_db_key(&contract));
                            } else {
                                match prev {
                                    Some(v) => batch.put(storage_db_key(&contract, &key), v),
                                    None => batch.delete(storage_db_key(&contract, &key)),
                                }
                            }
                        }
                        UndoOp::Delete {
                            contract,
                            key,
                            prev,
                        } => {
                            batch.put(storage_db_key(&contract, &key), prev);
                        }
                    }
                }
                batch.delete(height_db_key(h));
                batch.delete(undo_db_key(h));
                self.db
                    .write(batch)
                    .context("rocksdb rollback batch failed")?;
            }
        }

        // Restore metadata to target.
        if let Some(rec) = self.height_history().last() {
            self.db
                .put(b"m:h", rec.height.to_be_bytes())
                .context("put m:h failed")?;
            self.db
                .put(b"m:b", rec.zcash_block_hash.0)
                .context("put m:b failed")?;
            self.db
                .put(b"m:r", rec.state_root.0)
                .context("put m:r failed")?;
        } else {
            self.db.delete(b"m:h").context("del m:h failed")?;
            self.db.delete(b"m:b").context("del m:b failed")?;
            self.db.delete(b"m:r").context("del m:r failed")?;
        }
        Ok(())
    }

    fn save_executions(&mut self, executions: &[Execution]) -> Result<()> {
        let mut batch = rocksdb::WriteBatch::default();
        for e in executions {
            let json = serde_json::to_vec(e).context("serialize execution")?;
            batch.put(exec_db_key(&e.txid), json);
            // Index marker: x<height> = height for block query (no value needed,
            // but we store empty to keep a clean iteration).
            batch.put(exec_height_db_key(e.block_height), []);
        }
        self.db.write(batch).context("rocksdb save executions")
    }

    fn execution(&self, txid: &TxId) -> Option<Execution> {
        let bytes = self.db.get(exec_db_key(txid)).ok().flatten()?;
        serde_json::from_slice(&bytes).ok()
    }

    fn executions_at_height(&self, height: BlockHeight) -> Vec<Execution> {
        // Scan all `e<txid>` keys and filter by height.
        let mut out = Vec::new();
        let iter = self.db.iterator(rocksdb::IteratorMode::From(
            b"e",
            rocksdb::Direction::Forward,
        ));
        for item in iter.flatten() {
            let (key, value) = item;
            if key.is_empty() || key[0] != b'e' {
                continue;
            }
            if let Ok(e) = serde_json::from_slice::<Execution>(&value) {
                if e.block_height == height {
                    out.push(e);
                }
            }
        }
        out
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn root_suite<S: StateStore>(mut s: S) {
        let r0 = s.compute_root();

        let cid = ContractId([1u8; 32]);
        let code_hash = CodeHash([0xAB; 32]);
        let commit = BlockCommit {
            height: 1,
            zcash_block_hash: BlockHash([0x01; 32]),
            deploys: vec![(cid, code_hash, vec![0x00, 0x61, 0x73, 0x6D])],
            upserts: vec![],
            deletes: vec![],
        };
        let r1 = s.commit_block(commit).unwrap();
        assert_ne!(r0, r1, "deploy must change root");

        let c2 = BlockCommit {
            height: 2,
            zcash_block_hash: BlockHash([0x02; 32]),
            deploys: vec![],
            upserts: vec![(cid, b"key".to_vec(), b"value".to_vec())],
            deletes: vec![],
        };
        let r2 = s.commit_block(c2).unwrap();
        assert_ne!(r1, r2, "storage write must change root");
        assert_eq!(s.storage_get(&cid, b"key"), Some(b"value".to_vec()));

        // Rollback height 2 → root back to r1, storage key gone.
        s.rollback_to(1).unwrap();
        assert_eq!(s.compute_root(), r1, "rollback must restore root");
        assert_eq!(s.storage_get(&cid, b"key"), None);
        assert_eq!(s.indexed_height(), Some(1));

        // Rollback height 1 → empty.
        s.rollback_to(0).unwrap();
        assert_eq!(s.compute_root(), r0);
        assert!(!s.has_contract(&cid));
        assert_eq!(s.indexed_height(), None);
    }

    #[test]
    fn memory_root_and_rollback_suite() {
        root_suite(MemoryState::new());
    }

    #[test]
    fn rocks_root_and_rollback_suite() {
        let dir = tempfile_dir();
        root_suite(RocksState::open(&dir).unwrap());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn rocks_state_persists_across_reopen() {
        let dir = tempfile_dir();
        let cid = ContractId([9u8; 32]);
        let code_hash = CodeHash([0x11; 32]);

        let root = {
            let mut s = RocksState::open(&dir).unwrap();
            s.commit_block(BlockCommit {
                height: 1,
                zcash_block_hash: BlockHash([0x22; 32]),
                deploys: vec![(cid, code_hash, vec![0x00, 0x61, 0x73, 0x6D])],
                upserts: vec![(cid, b"counter".to_vec(), 2u64.to_be_bytes().to_vec())],
                deletes: vec![],
            })
            .unwrap()
        };

        let s = RocksState::open(&dir).unwrap();
        assert_eq!(s.indexed_height(), Some(1));
        assert_eq!(s.indexed_block_hash(), Some(BlockHash([0x22; 32])));
        let (_, wasm) = s.get_contract(&cid).unwrap();
        assert_eq!(wasm, vec![0x00, 0x61, 0x73, 0x6D]);
        assert_eq!(
            s.storage_get(&cid, b"counter"),
            Some(2u64.to_be_bytes().to_vec())
        );
        assert_eq!(s.compute_root(), root);
        std::fs::remove_dir_all(&dir).ok();
    }

    fn tempfile_dir() -> std::path::PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!(
            "zalkanes-state-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&p).unwrap();
        p
    }
}
