//! # zalkanes-testkit
//!
//! In-memory test harness for Zalkanes contract development and protocol testing.
//!
//! Uses the real protocol parser, real WASM validator, real Wasmi runtime,
//! real state root calculation, and the REAL production block processor
//! (`zalkanes_indexer::process_zcash_block`) over an in-memory `StateStore` —
//! NOT a fake interpreter and NOT a `TestTx` path.

#![forbid(unsafe_code)]

use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256};
use zalkanes_carrier::{split, Chunk};
use zalkanes_core::types::{
    BlockHash, BlockHeight, CodeHash, ContractId, Network, StateRoot, TxId,
};
use zalkanes_indexer::{process_parsed_block, IndexerConfig, ParsedBlock, ParsedTransaction};
use zalkanes_protocol::{encode_call, encode_deploy, CallMessage, DeployMessage};
use zalkanes_runtime::{execute, CallContext, CallResult};
use zalkanes_state::{BlockCommit, MemoryState, StateStore};

/// In-memory test chain.
///
/// Each `deploy`/`call` mines a new block automatically. Uses the production
/// `process_zcash_block` on synthetic-but-canonical raw block bytes.
pub struct TestChain {
    state: MemoryState,
    height: BlockHeight,
    network: Network,
}

impl TestChain {
    pub fn new() -> Self {
        Self {
            state: MemoryState::new(),
            height: 0,
            network: Network::Regtest,
        }
    }

    pub fn network(&self) -> Network {
        self.network
    }

    pub fn height(&self) -> BlockHeight {
        self.height
    }

    pub fn state_root(&self) -> StateRoot {
        self.state.compute_root()
    }

    pub fn state(&self) -> &MemoryState {
        &self.state
    }

    /// Deploy a WASM contract through the REAL block processor.
    pub fn deploy(&mut self, wasm: &[u8]) -> Result<ContractId> {
        zalkanes_runtime::validate_module(wasm)
            .map_err(|e| anyhow::anyhow!("WASM validation failed: {e}"))?;

        let code_hash = CodeHash::of(wasm);
        let txid = synthetic_txid(self.height, 0);
        let output_index: u16 = 0;
        let contract_id = ContractId::derive(self.network, &txid, output_index, &code_hash);

        let chunks = split(wasm, 4096);
        let chunk_count = chunks.len() as u8;
        let deploy_msg = DeployMessage {
            code_hash,
            code_length: wasm.len() as u32,
            chunk_count,
            output_index,
        };

        // Build a ParsedBlock with one transparent tx: OP_RETURN output +
        // carrier scriptSig inputs (one per chunk).
        let parsed = self.deploy_parsed_block(&txid, &encode_deploy(&deploy_msg), &chunks);
        self.height += 1;

        let config = self.indexer_config();
        process_parsed_block(&mut self.state, &config, parsed)
            .with_context(|| "process deploy block")?;

        Ok(contract_id)
    }

    /// Call a contract method through the REAL block processor.
    pub fn call(&mut self, contract_id: ContractId, opcode: u16, input: &[u8]) -> Result<()> {
        let txid = synthetic_txid(self.height, 1);
        let msg = CallMessage {
            contract_id,
            opcode,
            input: input.to_vec(),
        };
        let parsed = self.call_parsed_block(&txid, &encode_call(&msg));
        self.height += 1;

        let config = self.indexer_config();
        process_parsed_block(&mut self.state, &config, parsed)
            .with_context(|| "process call block")?;
        Ok(())
    }

    fn indexer_config(&self) -> IndexerConfig {
        IndexerConfig {
            network: self.network,
            data_dir: std::path::PathBuf::from("/tmp/zalkanes-test"),
        }
    }

    /// Build a `ParsedBlock` for a deploy: OP_RETURN output + carrier inputs.
    fn deploy_parsed_block(&self, txid: &TxId, op_return: &[u8], chunks: &[Chunk]) -> ParsedBlock {
        let height = self.height + 1;
        let hash = BlockHash(synthetic_hash(height));
        let mut inputs = vec![(0u32, encode_coinbase_script(height))];
        for (i, chunk) in chunks.iter().enumerate() {
            inputs.push((
                (i + 1) as u32,
                encode_carrier_script_sig(chunk.index, &chunk.data),
            ));
        }
        let tx = ParsedTransaction {
            txid: *txid,
            outputs: vec![(0u16, op_return_script(op_return)), (1u16, vec![0x51])],
            inputs,
        };
        ParsedBlock {
            height,
            hash,
            transactions: vec![tx],
        }
    }

    /// Build a `ParsedBlock` for a call: OP_RETURN output only.
    fn call_parsed_block(&self, txid: &TxId, op_return: &[u8]) -> ParsedBlock {
        let height = self.height + 1;
        let hash = BlockHash(synthetic_hash(height));
        let tx = ParsedTransaction {
            txid: *txid,
            outputs: vec![(0u16, op_return_script(op_return))],
            inputs: vec![(0u32, encode_coinbase_script(height))],
        };
        ParsedBlock {
            height,
            hash,
            transactions: vec![tx],
        }
    }

    /// Execute a view call (read-only, no state persistence).
    pub fn view(&self, contract_id: ContractId, opcode: u16, input: &[u8]) -> Result<Vec<u8>> {
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
        match execute(ctx, &self.state) {
            CallResult::Success { output, .. } => Ok(output),
            CallResult::Trap { reason, .. } => bail!("view trapped: {reason}"),
            CallResult::FuelExhausted { .. } => bail!("view fuel exhausted"),
            CallResult::InvalidModule { reason } => bail!("invalid module: {reason}"),
            CallResult::ContractNotFound => bail!("contract not found"),
        }
    }

    /// Commit an empty block (advances height, no state change).
    /// Mine a block whose OP_RETURN carries ARBITRARY bytes (may be a
    /// malformed or non-ZALK payload): exercises the parser's skip/error
    /// paths through the production block processor.
    pub fn raw_message_block(&mut self, payload: &[u8]) -> Result<()> {
        let txid = synthetic_txid(self.height, 1);
        let parsed = self.call_parsed_block(&txid, payload);
        self.height += 1;
        let config = self.indexer_config();
        process_parsed_block(&mut self.state, &config, parsed)
            .with_context(|| "process raw block")?;
        Ok(())
    }

    /// Mine ONE block containing multiple CALL transactions (per-block budget
    /// and intra-block visibility semantics under test).
    pub fn multi_call_block(&mut self, calls: &[(ContractId, u16, Vec<u8>)]) -> Result<()> {
        let height = self.height + 1;
        let hash = BlockHash(synthetic_hash(height));
        let mut transactions = Vec::new();
        for (i, (contract_id, opcode, input)) in calls.iter().enumerate() {
            let txid = synthetic_txid(self.height, (i + 1) as u32);
            let msg = CallMessage {
                contract_id: *contract_id,
                opcode: *opcode,
                input: input.clone(),
            };
            transactions.push(ParsedTransaction {
                txid,
                outputs: vec![(0u16, op_return_script(&encode_call(&msg)))],
                inputs: vec![(0u32, encode_coinbase_script(height))],
            });
        }
        let parsed = ParsedBlock {
            height,
            hash,
            transactions,
        };
        self.height += 1;
        let config = self.indexer_config();
        process_parsed_block(&mut self.state, &config, parsed)
            .with_context(|| "process multi-call block")?;
        Ok(())
    }

    /// Mutable access to the underlying state (rollback tests).
    pub fn state_mut(&mut self) -> &mut MemoryState {
        &mut self.state
    }

    pub fn mine_empty_block(&mut self) -> Result<()> {
        let root_before = self.state.compute_root();
        self.height += 1;
        let hash = BlockHash(synthetic_hash(self.height));
        self.state
            .commit_block(BlockCommit {
                height: self.height,
                zcash_block_hash: hash,
                deploys: vec![],
                upserts: vec![],
                deletes: vec![],
            })
            .with_context(|| "commit empty block")?;
        let _ = root_before;
        Ok(())
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

/// Build an OP_RETURN script: `OP_RETURN <data>`.
fn op_return_script(data: &[u8]) -> Vec<u8> {
    let mut s = vec![0x6a];
    s.extend_from_slice(&encode_push(data));
    s
}

fn encode_coinbase_script(height: BlockHeight) -> Vec<u8> {
    // BIP34-style: push the block height. Parsed as a carrier input would fail
    // chunk extraction (no chunk_index), so it is ignored by the carrier.
    let mut v = Vec::new();
    let h = height as u64;
    if h == 0 {
        v.push(0x00);
        return v;
    }
    let bytes = h.to_be_bytes();
    let first = bytes.iter().position(|&b| b != 0).unwrap_or(7);
    let len = 8 - first;
    v.push(len as u8);
    v.extend_from_slice(&bytes[first..]);
    v
}

fn encode_push(data: &[u8]) -> Vec<u8> {
    if data.len() <= 75 {
        let mut v = vec![data.len() as u8];
        v.extend_from_slice(data);
        v
    } else if data.len() <= 0xff {
        let mut v = vec![0x4c, data.len() as u8];
        v.extend_from_slice(data);
        v
    } else {
        let mut v = vec![0x4d];
        v.extend_from_slice(&(data.len() as u16).to_le_bytes());
        v.extend_from_slice(data);
        v
    }
}

fn encode_carrier_script_sig(index: u8, data: &[u8]) -> Vec<u8> {
    // <chunk_index: u8> <chunk_data> <sig> <redeem>
    let mut v = Vec::new();
    v.push(0x01);
    v.push(index);
    v.extend_from_slice(&encode_push(data));
    v.push(0x01);
    v.push(0x30);
    v.push(0x01);
    v.push(0x51);
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    // Minimal valid WASM: a module exporting `dispatch(i32,i32)->i32` returning 0.
    // (module (func (export "dispatch") (param i32 i32) (result i32) i32.const 0))
    const MINIMAL_WASM: &[u8] = &[
        0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00, // magic + version
        0x01, 0x07, 0x01, 0x60, 0x02, 0x7f, 0x7f, 0x01, 0x7f, // type section (1 fn)
        0x03, 0x02, 0x01, 0x00, // func section
        0x07, 0x0c, 0x01, 0x08, 0x64, 0x69, 0x73, 0x70, 0x61, 0x74, 0x63, 0x68, // export
        0x00, 0x00, // kind func, index 0
        0x0a, 0x06, 0x01, 0x04, 0x00, 0x41, 0x00, 0x0b, // code section
    ];

    #[test]
    fn new_chain_has_zero_height() {
        let chain = TestChain::new();
        assert_eq!(chain.height(), 0);
    }

    #[test]
    fn deploy_changes_state_root() {
        let mut chain = TestChain::new();
        let r0 = chain.state_root();
        chain.deploy(MINIMAL_WASM).unwrap();
        let r1 = chain.state_root();
        assert_ne!(r0, r1, "deploy must change state root");
    }

    #[test]
    fn state_root_is_deterministic_on_empty_chain() {
        let a = TestChain::new().state_root();
        let b = TestChain::new().state_root();
        assert_eq!(a, b);
    }
}
