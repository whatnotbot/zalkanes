//! # zalkanes-indexer
//!
//! Real block processing: deserialize Zcash blocks, extract Zalkanes messages
//! and carrier chunks, execute WASM, and atomically persist state.
//!
//! Production path is `process_zcash_block` over a `&mut dyn StateStore`.
//! The `TestTx`/`MemoryIndexer` types live only in the testkit.

#![forbid(unsafe_code)]

mod parse;

use anyhow::Result;
use tracing::{debug, info, warn};
use zalkanes_carrier::{extract_op_return_data, Chunk};
use zalkanes_core::{
    consensus::{MAINNET_ACTIVATION_HEIGHT, REGTEST_ACTIVATION_HEIGHT, TESTNET_ACTIVATION_HEIGHT},
    types::{BlockHash, BlockHeight, ContractId, Execution, Network, StateRoot},
};
use zalkanes_protocol::{parse_op_return, Message};
use zalkanes_runtime::{execute, CallContext, CallResult, StorageWrite};
use zalkanes_state::{BlockCommit, StateStore};

pub use parse::{parse_block, ParsedBlock, ParsedTransaction};

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

/// Executions produced by processing one block.
#[derive(Debug, Clone)]
pub struct BlockExecution {
    pub height: BlockHeight,
    pub block_hash: BlockHash,
    pub state_root_before: StateRoot,
    pub state_root_after: StateRoot,
    pub executions: Vec<Execution>,
    /// Deployments accepted this block.
    pub deployed: Vec<ContractId>,
}

/// Process a raw Zcash block against persistent state.
///
/// This is the production block processor. It:
/// 1. deserializes the real Zcash block;
/// 2. for each transparent tx, extracts OP_RETURN protocol messages and
///    carrier chunks;
/// 3. reconstructs + validates deployed WASM against the committed code hash;
/// 4. executes CALLs against a write buffer, capturing per-call state roots;
/// 5. atomically commits all writes + metadata in one `BlockCommit`.
pub fn process_zcash_block(
    store: &mut dyn StateStore,
    config: &IndexerConfig,
    height: BlockHeight,
    block_hash: BlockHash,
    raw_block: &[u8],
) -> Result<BlockExecution> {
    let parsed = parse_block(raw_block, height, block_hash, config.network)?;
    process_parsed_block(store, config, parsed)
}

/// Process an already-parsed block against persistent state.
///
/// Exposed separately from [`process_zcash_block`] so the testkit and unit
/// tests can exercise the exact production execution + commit logic against a
/// `ParsedBlock` without hand-serializing Zcash block bytes. Production always
/// goes through `process_zcash_block` (raw bytes → parser → this function).
pub fn process_parsed_block(
    store: &mut dyn StateStore,
    config: &IndexerConfig,
    parsed: ParsedBlock,
) -> Result<BlockExecution> {
    let activation = match config.activation_height() {
        Some(h) => h,
        None => {
            debug!(
                height = parsed.height,
                "no activation height set; skipping block"
            );
            return Ok(BlockExecution {
                height: parsed.height,
                block_hash: parsed.hash,
                state_root_before: store.compute_root(),
                state_root_after: store.compute_root(),
                executions: vec![],
                deployed: vec![],
            });
        }
    };

    if parsed.height < activation {
        debug!(
            height = parsed.height,
            "below activation height; skipping block"
        );
        return Ok(BlockExecution {
            height: parsed.height,
            block_hash: parsed.hash,
            state_root_before: store.compute_root(),
            state_root_after: store.compute_root(),
            executions: vec![],
            deployed: vec![],
        });
    }

    let state_root_before = store.compute_root();

    let mut commit = BlockCommit {
        height: parsed.height,
        zcash_block_hash: parsed.hash,
        deploys: Vec::new(),
        upserts: Vec::new(),
        deletes: Vec::new(),
    };
    let mut executions = Vec::new();
    let mut deployed = Vec::new();

    for tx in &parsed.transactions {
        process_transaction(
            store,
            config,
            &mut commit,
            &mut executions,
            &mut deployed,
            tx,
        )?;
    }

    let state_root_after = store.commit_block(commit)?;
    store.save_executions(&executions)?;
    info!(
        height = parsed.height,
        before = %state_root_before,
        after = %state_root_after,
        executions = executions.len(),
        "block indexed"
    );

    Ok(BlockExecution {
        height: parsed.height,
        block_hash: parsed.hash,
        state_root_before,
        state_root_after,
        executions,
        deployed,
    })
}

fn process_transaction(
    store: &mut dyn StateStore,
    config: &IndexerConfig,
    commit: &mut BlockCommit,
    executions: &mut Vec<Execution>,
    deployed: &mut Vec<ContractId>,
    tx: &ParsedTransaction,
) -> Result<()> {
    // Iterate transparent outputs; find Zalkanes OP_RETURN messages.
    for (out_idx, script_pubkey) in &tx.outputs {
        let Some(payload) = extract_op_return_data(script_pubkey) else {
            continue;
        };
        match parse_op_return(&payload) {
            Ok(Some(Message::Deploy(deploy))) => {
                if deploy.output_index != *out_idx {
                    debug!(txid = %tx.txid, "deploy output_index mismatch; skipping");
                    continue;
                }
                handle_deploy(store, config, commit, deployed, tx, deploy)?;
            }
            Ok(Some(Message::Call(call))) => {
                handle_call(store, commit, executions, tx, call)?;
            }
            Ok(None) => {}
            Err(e) => {
                debug!(txid = %tx.txid, error = %e, "parse error in OP_RETURN; skipping");
            }
        }
    }
    Ok(())
}

fn handle_deploy(
    store: &mut dyn StateStore,
    config: &IndexerConfig,
    commit: &mut BlockCommit,
    deployed: &mut Vec<ContractId>,
    tx: &ParsedTransaction,
    deploy: zalkanes_protocol::DeployMessage,
) -> Result<()> {
    // Collect carrier chunks from transparent inputs, in order.
    let mut chunks: Vec<Chunk> = Vec::new();
    for (_idx, script_sig) in &tx.inputs {
        if let Some((index, data)) = zalkanes_carrier::chunk_from_script_sig(script_sig) {
            chunks.push(Chunk { index, data });
        }
    }

    match zalkanes_carrier::reconstruct(
        &chunks,
        deploy.chunk_count,
        deploy.code_length,
        &deploy.code_hash,
    ) {
        Ok(wasm) => {
            let contract_id = ContractId::derive(
                config.network,
                &tx.txid,
                deploy.output_index,
                &deploy.code_hash,
            );
            if store.has_contract(&contract_id) {
                debug!(%contract_id, "contract already deployed; ignoring");
                return Ok(());
            }
            if let Err(e) = zalkanes_runtime::validate_module(&wasm) {
                warn!(%contract_id, reason = %e, "WASM validation failed; rejecting deploy");
                return Ok(());
            }
            commit.deploys.push((contract_id, deploy.code_hash, wasm));
            deployed.push(contract_id);
            info!(%contract_id, txid = %tx.txid, "contract deployed");
        }
        Err(e) => {
            debug!(txid = %tx.txid, error = %e, "carrier reconstruction failed; rejecting deploy");
        }
    }
    Ok(())
}

fn handle_call(
    store: &mut dyn StateStore,
    commit: &mut BlockCommit,
    executions: &mut Vec<Execution>,
    tx: &ParsedTransaction,
    call: zalkanes_protocol::CallMessage,
) -> Result<()> {
    let ctx = CallContext {
        contract_id: call.contract_id,
        caller: None,
        txid: tx.txid,
        block_height: 0, // patched below; CallContext needs height — see note
        opcode: call.opcode,
        input: call.input.clone(),
        fuel_limit: zalkanes_core::consensus::MAX_FUEL_PER_CALL,
        depth: 0,
    };
    // Set the block height from the current commit.
    let ctx = CallContext {
        block_height: commit.height,
        ..ctx
    };

    let root_before = store.compute_root();
    let result = execute(ctx, store);

    // Apply writes from a successful call into the pending block commit.
    let (success, fuel_used, output, error, writes) = match result {
        CallResult::Success {
            output,
            fuel_used,
            writes,
        } => (true, fuel_used, output, None, writes),
        CallResult::Trap { reason, fuel_used } => (false, fuel_used, vec![], Some(reason), vec![]),
        CallResult::FuelExhausted { fuel_used } => (
            false,
            fuel_used,
            vec![],
            Some("fuel exhausted".into()),
            vec![],
        ),
        CallResult::InvalidModule { reason } => (false, 0, vec![], Some(reason), vec![]),
        CallResult::ContractNotFound => {
            (false, 0, vec![], Some("contract not found".into()), vec![])
        }
    };

    for w in writes {
        apply_write(commit, w);
    }

    // Compute post-call root for the execution record.
    let root_after = compute_root_after(store, commit);

    executions.push(Execution {
        txid: tx.txid,
        contract_id: call.contract_id,
        opcode: call.opcode,
        success,
        fuel_used,
        return_data: output,
        error,
        state_root_before: root_before,
        state_root_after: root_after,
        block_height: commit.height,
        block_hash: commit.zcash_block_hash,
    });

    if success {
        debug!(contract = %call.contract_id, fuel_used, "call succeeded");
    } else {
        debug!(contract = %call.contract_id, "call failed");
    }
    Ok(())
}

fn apply_write(commit: &mut BlockCommit, w: StorageWrite) {
    match w {
        StorageWrite::Set(cid, key, value) => commit.upserts.push((cid, key, value)),
        StorageWrite::Delete(cid, key) => commit.deletes.push((cid, key)),
    }
}

/// Compute the root the store *would* have after applying the pending commit.
///
/// Used only for execution records (before/after roots); the authoritative
/// root is computed and persisted inside `commit_block`.
fn compute_root_after(store: &dyn StateStore, commit: &BlockCommit) -> StateRoot {
    // Reconstruct a MemoryState view is O(n) and acceptable for now, but the
    // authoritative root comes from commit_block. For the execution record we
    // approximate by hashing a combined leaf set. To stay exact, we instead
    // return the store's current root plus the pending writes via a lightweight
    // snapshot. NOTE: for correctness we compute the exact root below.
    let _ = (store, commit);
    // Fall back to a snapshot approach implemented in state:
    zalkanes_state::projected_root(store, commit)
}
