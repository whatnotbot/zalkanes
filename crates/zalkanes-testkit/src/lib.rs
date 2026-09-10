//! # zalkanes-testkit
//!
//! In-memory test harness for Zalkanes contract development and protocol testing.
//!
//! Uses the real protocol parser, real WASM validator, real Wasmi runtime,
//! real state root calculation — NOT a fake interpreter.
//!
//! Example:
//! ```ignore
//! # use zalkanes_testkit::TestChain;
//! let mut chain = TestChain::new();
//! let contract_id = chain.deploy(include_bytes!("counter.wasm")).unwrap();
//! chain.call(contract_id, 1, &[]).unwrap();
//! chain.call(contract_id, 1, &[]).unwrap();
//! let result = chain.view(contract_id, 2, &[]).unwrap();
//! assert_eq!(result, 2u64.to_be_bytes());
//! ```

#![forbid(unsafe_code)]

use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256};
use zalkanes_carrier::{split, Chunk};
use zalkanes_core::types::{
    BlockHash, BlockHeight, CodeHash, ContractId, Network, StateRoot, TxId,
};
use zalkanes_indexer::{IndexerConfig, MemoryIndexer, TestTx};
use zalkanes_protocol::{encode_call, encode_deploy, CallMessage, DeployMessage};
use zalkanes_runtime::{execute, CallContext, CallResult};
use zalkanes_state::MemoryState;

/// In-memory test chain.
///
/// Each `deploy`/`call` mines a new block automatically.
pub struct TestChain {
    indexer: MemoryIndexer,
    height: BlockHeight,
}

impl TestChain {
    pub fn new() -> Self {
        let config = IndexerConfig {
            network: Network::Regtest,
            data_dir: std::path::PathBuf::from("/tmp/zalkanes-test"),
        };
        Self {
            indexer: MemoryIndexer::new(config),
            height: 0,
        }
    }

    pub fn network(&self) -> Network {
        self.indexer.config.network
    }

    pub fn height(&self) -> BlockHeight {
        self.height
    }

    pub fn state_root(&self) -> StateRoot {
        self.indexer.current_state_root()
    }

    /// Deploy a WASM contract. Returns the ContractId.
    pub fn deploy(&mut self, wasm: &[u8]) -> Result<ContractId> {
        // Validate first
        zalkanes_runtime::validate_module(wasm)
            .map_err(|e| anyhow::anyhow!("WASM validation failed: {}", e))?;

        let code_hash = CodeHash::of(wasm);
        let txid = synthetic_txid(self.height, 0);
        let output_index: u16 = 0;

        let contract_id = ContractId::derive(Network::Regtest, &txid, output_index, &code_hash);

        // Split into chunks (4 KiB each)
        let chunks = split(wasm, 4096);
        let chunk_count = chunks.len() as u8;

        let deploy_msg = DeployMessage {
            code_hash,
            code_length: wasm.len() as u32,
            chunk_count,
            output_index,
        };
        let op_return_bytes = encode_deploy(&deploy_msg);

        let tx = TestTx {
            txid,
            op_returns: vec![(0, op_return_bytes)],
            carrier_chunks: chunks
                .iter()
                .map(|c: &Chunk| (c.index, c.data.clone()))
                .collect(),
        };

        self.mine_block(vec![tx])?;
        Ok(contract_id)
    }

    /// Call a contract method. Mines the call into a new block.
    pub fn call(&mut self, contract_id: ContractId, opcode: u16, input: &[u8]) -> Result<Vec<u8>> {
        let txid = synthetic_txid(self.height, 1);
        let msg = CallMessage {
            contract_id,
            opcode,
            input: input.to_vec(),
        };
        let op_return_bytes = encode_call(&msg);
        let tx = TestTx {
            txid,
            op_returns: vec![(0, op_return_bytes)],
            carrier_chunks: vec![],
        };
        self.mine_block(vec![tx])?;
        Ok(vec![]) // actual return value comes from view
    }

    /// Execute a view call (read-only, no state persistence).
    pub fn view(&self, contract_id: ContractId, opcode: u16, input: &[u8]) -> Result<Vec<u8>> {
        // Clone state for read-only overlay
        let mut overlay = MemoryState::new();
        // Copy contracts
        for (k, v) in &self.indexer.state.contracts {
            overlay.contracts.insert(*k, v.clone());
        }
        // Copy storage (read-only overlay discards any writes)
        for (k, v) in &self.indexer.state.storage {
            overlay.storage.insert(k.clone(), v.clone());
        }

        let ctx = CallContext {
            contract_id,
            caller: None,
            txid: TxId([0u8; 32]),
            block_height: self.height,
            opcode,
            input: input.to_vec(),
            fuel_limit: zalkanes_core::consensus::MAX_FUEL_PER_CALL,
            depth: 0,
        };

        match execute(ctx, &mut overlay) {
            CallResult::Success { output, .. } => Ok(output),
            CallResult::Trap { reason, .. } => bail!("view trapped: {}", reason),
            CallResult::FuelExhausted { .. } => bail!("view fuel exhausted"),
            CallResult::InvalidModule { reason } => bail!("invalid module: {}", reason),
            CallResult::ContractNotFound => bail!("contract not found"),
        }
    }

    fn mine_block(&mut self, txs: Vec<TestTx>) -> Result<()> {
        self.height += 1;
        let hash = BlockHash(synthetic_hash(self.height));
        self.indexer
            .process_block(self.height, hash, &txs)
            .with_context(|| format!("failed to process block at height {}", self.height))
    }
}

impl Default for TestChain {
    fn default() -> Self {
        Self::new()
    }
}

fn synthetic_txid(height: BlockHeight, tx_index: u32) -> TxId {
    let mut data = [0u8; 8];
    data[0..4].copy_from_slice(&height.to_be_bytes());
    data[4..8].copy_from_slice(&tx_index.to_be_bytes());
    let digest = Sha256::digest(data);
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest);
    TxId(out)
}

fn synthetic_hash(height: BlockHeight) -> [u8; 32] {
    let digest = Sha256::digest(height.to_be_bytes());
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_chain_has_zero_height() {
        let chain = TestChain::new();
        assert_eq!(chain.height(), 0);
    }

    #[test]
    fn state_root_is_deterministic_on_empty_chain() {
        let a = TestChain::new().state_root();
        let b = TestChain::new().state_root();
        assert_eq!(a, b);
    }
}
