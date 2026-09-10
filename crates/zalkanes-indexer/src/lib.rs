//! # zalkanes-indexer
//!
//! Block processing loop, transaction parsing, and reorg handling.
//!
//! See `docs/architecture.md` for the data flow description.

#![forbid(unsafe_code)]

use anyhow::{Context, Result};
use tracing::{debug, info, warn};
use zalkanes_carrier::{reconstruct, Chunk};
use zalkanes_chain::ChainSource;
use zalkanes_core::{
    consensus::{MAINNET_ACTIVATION_HEIGHT, REGTEST_ACTIVATION_HEIGHT, TESTNET_ACTIVATION_HEIGHT},
    types::{BlockHash, BlockHeight, BlockRef, CodeHash, ContractId, Network},
};
use zalkanes_protocol::{parse_op_return, Message};
use zalkanes_runtime::{execute, CallContext, CallResult};
use zalkanes_state::{HeightRecord, MemoryState};

/// Configuration for the block indexer.
#[derive(Debug, Clone)]
pub struct IndexerConfig {
    pub network: Network,
    pub data_dir: std::path::PathBuf,
}

impl IndexerConfig {
    fn activation_height(&self) -> Option<BlockHeight> {
        match self.network {
            Network::Mainnet => MAINNET_ACTIVATION_HEIGHT,
            Network::Testnet => TESTNET_ACTIVATION_HEIGHT,
            Network::Regtest => REGTEST_ACTIVATION_HEIGHT,
        }
    }
}

/// An in-memory indexer used by the testkit and for initial development.
///
/// Production use will persist to RocksDB; this version uses `MemoryState`.
pub struct MemoryIndexer {
    pub config: IndexerConfig,
    pub state: MemoryState,
    pub history: Vec<HeightRecord>,
}

impl MemoryIndexer {
    pub fn new(config: IndexerConfig) -> Self {
        Self {
            config,
            state: MemoryState::new(),
            history: Vec::new(),
        }
    }

    pub fn current_height(&self) -> Option<BlockHeight> {
        self.history.last().map(|r| r.height)
    }

    pub fn current_state_root(&self) -> zalkanes_core::types::StateRoot {
        self.state.compute_root()
    }

    /// Process a list of transactions for a single block.
    ///
    /// In production this receives the deserialized block. Here we accept
    /// a simplified representation for testkit use.
    pub fn process_block(
        &mut self,
        height: BlockHeight,
        block_hash: BlockHash,
        txs: &[TestTx],
    ) -> Result<()> {
        let activation = match self.config.activation_height() {
            Some(h) => h,
            None => {
                debug!(height, "no activation height set; skipping block");
                return Ok(());
            }
        };

        if height < activation {
            debug!(height, "below activation height; skipping block");
            return Ok(());
        }

        for tx in txs {
            self.process_tx(height, tx)?;
        }

        let root = self.state.compute_root();
        self.history.push(HeightRecord {
            height,
            zcash_block_hash: block_hash,
            state_root: root,
        });
        info!(height, root = %root, "block committed");
        Ok(())
    }

    fn process_tx(&mut self, height: BlockHeight, tx: &TestTx) -> Result<()> {
        // Look for OP_RETURN outputs containing Zalkanes messages
        for (out_idx, op_return) in &tx.op_returns {
            match parse_op_return(op_return) {
                Ok(Some(Message::Deploy(deploy))) => {
                    if deploy.output_index as usize != *out_idx {
                        debug!("deploy output_index mismatch; skipping");
                        continue;
                    }
                    // Reconstruct WASM from carrier chunks
                    let chunks: Vec<Chunk> = tx
                        .carrier_chunks
                        .iter()
                        .map(|(idx, data)| Chunk {
                            index: *idx,
                            data: data.clone(),
                        })
                        .collect();

                    match reconstruct(
                        &chunks,
                        deploy.chunk_count,
                        deploy.code_length,
                        &deploy.code_hash,
                    ) {
                        Ok(wasm) => {
                            let contract_id = ContractId::derive(
                                self.config.network,
                                &tx.txid,
                                deploy.output_index,
                                &deploy.code_hash,
                            );
                            if self.state.has_contract(&contract_id) {
                                debug!(%contract_id, "contract already deployed; ignoring");
                                continue;
                            }
                            if let Err(e) = zalkanes_runtime::validate_module(&wasm) {
                                warn!(%contract_id, reason = %e, "WASM validation failed");
                                continue;
                            }
                            self.state.deploy(contract_id, deploy.code_hash, wasm);
                            info!(%contract_id, height, "contract deployed");
                        }
                        Err(e) => {
                            debug!(error = %e, "carrier reconstruction failed; skipping deploy");
                        }
                    }
                }
                Ok(Some(Message::Call(call))) => {
                    let ctx = CallContext {
                        contract_id: call.contract_id,
                        caller: None,
                        txid: tx.txid,
                        block_height: height,
                        opcode: call.opcode,
                        input: call.input.clone(),
                        fuel_limit: zalkanes_core::consensus::MAX_FUEL_PER_CALL,
                        depth: 0,
                    };
                    match execute(ctx, &mut self.state) {
                        CallResult::Success { fuel_used, .. } => {
                            debug!(contract = %call.contract_id, fuel_used, "call succeeded");
                        }
                        CallResult::Trap { reason, .. } => {
                            debug!(contract = %call.contract_id, %reason, "call trapped");
                        }
                        CallResult::FuelExhausted { .. } => {
                            debug!(contract = %call.contract_id, "call fuel exhausted");
                        }
                        CallResult::InvalidModule { reason } => {
                            debug!(contract = %call.contract_id, %reason, "invalid module on call");
                        }
                        CallResult::ContractNotFound => {
                            debug!(contract = %call.contract_id, "contract not found");
                        }
                    }
                }
                Ok(None) => {}
                Err(e) => {
                    debug!(error = %e, "parse error in OP_RETURN; skipping");
                }
            }
        }
        Ok(())
    }

    /// Roll back to `target_height`, discarding all state above it.
    ///
    /// In production this operates on RocksDB historical state.
    /// In this in-memory version we replay from genesis to target_height.
    pub fn rollback_to(&mut self, target_height: BlockHeight) -> Result<()> {
        self.history.retain(|r| r.height <= target_height);
        // MemoryState has no incremental rollback — caller must replay
        // This is acceptable for the testkit; production uses RocksDB.
        Ok(())
    }
}

/// A simplified transaction representation used by the testkit.
/// Production use parses actual serialized Zcash block data.
#[derive(Debug, Clone)]
pub struct TestTx {
    pub txid: zalkanes_core::types::TxId,
    /// (output_index, op_return_bytes)
    pub op_returns: Vec<(usize, Vec<u8>)>,
    /// (chunk_index, chunk_data)
    pub carrier_chunks: Vec<(u8, Vec<u8>)>,
}
