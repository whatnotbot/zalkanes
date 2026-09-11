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
    consensus::{
        MAINNET_ACTIVATION_HEIGHT, MAX_CARRIER_BYTES_PER_BLOCK, MAX_DEPLOY_BYTES_PER_BLOCK,
        MAX_FUEL_PER_CALL, MAX_FUEL_PER_ZCASH_BLOCK, MAX_FUEL_PER_ZCASH_TX,
        MAX_ZALK_MESSAGES_PER_BLOCK, MAX_ZALK_MESSAGES_PER_TX, REGTEST_ACTIVATION_HEIGHT,
        TESTNET_ACTIVATION_HEIGHT,
    },
    types::{BlockHash, BlockHeight, CodeHash, ContractId, Execution, Network, StateRoot},
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
    /// The block height at which Zalkanes protocol interpretation begins, for
    /// this network. `None` means never (mainnet, pre-audit).
    pub fn activation_height(&self) -> Option<BlockHeight> {
        match self.network {
            Network::Mainnet => MAINNET_ACTIVATION_HEIGHT,
            Network::Testnet => TESTNET_ACTIVATION_HEIGHT,
            Network::Regtest => REGTEST_ACTIVATION_HEIGHT,
        }
    }
}

/// Per-block resource accounting, enforcing the DoS bounds.
#[derive(Debug, Default)]
struct BlockLimits {
    fuel_used: u64,
    messages: u32,
    carrier_bytes: u64,
    deploy_bytes: u64,
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
    // Fast path: below the activation height a block has no protocol effect and
    // its state is provably empty, so we skip the (expensive) librustzcash
    // deserialization and commit an empty block that advances the indexer's
    // position and records the canonical block hash. This is deterministic: the
    // state root below activation is always the empty root.
    if let Some(activation) = config.activation_height() {
        if height < activation {
            let root = store.commit_block(BlockCommit {
                height,
                zcash_block_hash: block_hash,
                deploys: Vec::new(),
                upserts: Vec::new(),
                deletes: Vec::new(),
            })?;
            debug!(height, "below activation height; committed empty block");
            return Ok(BlockExecution {
                height,
                block_hash,
                state_root_before: root,
                state_root_after: root,
                executions: vec![],
                deployed: vec![],
            });
        }
    }

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
        // Below activation there is no protocol interpretation, but the
        // indexer's position must still advance so it can reach the activation
        // height. Commit an empty block (deterministic empty state root).
        let root = store.commit_block(BlockCommit {
            height: parsed.height,
            zcash_block_hash: parsed.hash,
            deploys: Vec::new(),
            upserts: Vec::new(),
            deletes: Vec::new(),
        })?;
        debug!(
            height = parsed.height,
            "below activation height; committing empty block"
        );
        return Ok(BlockExecution {
            height: parsed.height,
            block_hash: parsed.hash,
            state_root_before: root,
            state_root_after: root,
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
    let mut limits = BlockLimits::default();

    for tx in &parsed.transactions {
        process_transaction(
            store,
            config,
            &mut commit,
            &mut executions,
            &mut deployed,
            &mut limits,
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
    limits: &mut BlockLimits,
    tx: &ParsedTransaction,
) -> Result<()> {
    let mut tx_fuel_used = 0u64;
    let mut tx_messages = 0u32;

    // Iterate transparent outputs; find Zalkanes OP_RETURN messages.
    for (out_idx, script_pubkey) in &tx.outputs {
        let Some(payload) = extract_op_return_data(script_pubkey) else {
            continue;
        };
        let parsed = match parse_op_return(&payload) {
            Ok(Some(m)) => m,
            Ok(None) => continue,
            Err(e) => {
                debug!(txid = %tx.txid, error = %e, "parse error in OP_RETURN; skipping");
                continue;
            }
        };

        // Per-tx and per-block message caps.
        tx_messages += 1;
        limits.messages += 1;
        if tx_messages > MAX_ZALK_MESSAGES_PER_TX || limits.messages > MAX_ZALK_MESSAGES_PER_BLOCK {
            warn!(txid = %tx.txid, "ZALK message cap exceeded; skipping remaining messages");
            return Ok(());
        }

        match parsed {
            Message::Deploy(deploy) => {
                if deploy.output_index != *out_idx {
                    debug!(txid = %tx.txid, "deploy output_index mismatch; skipping");
                    continue;
                }
                // Account declared deploy bytes before reconstruction so a block
                // full of huge declared deploys is bounded.
                limits.carrier_bytes = limits
                    .carrier_bytes
                    .saturating_add(deploy.code_length as u64);
                limits.deploy_bytes = limits
                    .deploy_bytes
                    .saturating_add(deploy.code_length as u64);
                if limits.carrier_bytes > MAX_CARRIER_BYTES_PER_BLOCK
                    || limits.deploy_bytes > MAX_DEPLOY_BYTES_PER_BLOCK
                {
                    warn!(txid = %tx.txid, "per-block carrier/deploy byte cap exceeded; skipping deploy");
                    continue;
                }
                handle_deploy(store, config, commit, deployed, tx, deploy)?;
            }
            Message::Call(call) => {
                handle_call(
                    store,
                    commit,
                    executions,
                    tx,
                    limits,
                    &mut tx_fuel_used,
                    call,
                )?;
            }
            Message::CallCarrier(call) => {
                limits.carrier_bytes = limits
                    .carrier_bytes
                    .saturating_add(call.input_length as u64);
                if limits.carrier_bytes > MAX_CARRIER_BYTES_PER_BLOCK {
                    warn!(txid = %tx.txid, "per-block carrier byte cap exceeded; skipping carrier call");
                    continue;
                }
                handle_call_carrier(
                    store,
                    commit,
                    executions,
                    tx,
                    limits,
                    &mut tx_fuel_used,
                    call,
                )?;
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
    limits: &mut BlockLimits,
    tx_fuel_used: &mut u64,
    call: zalkanes_protocol::CallMessage,
) -> Result<()> {
    execute_call(
        store,
        commit,
        executions,
        tx,
        limits,
        tx_fuel_used,
        call.contract_id,
        call.opcode,
        call.input,
    )
}

fn handle_call_carrier(
    store: &mut dyn StateStore,
    commit: &mut BlockCommit,
    executions: &mut Vec<Execution>,
    tx: &ParsedTransaction,
    limits: &mut BlockLimits,
    tx_fuel_used: &mut u64,
    call: zalkanes_protocol::CallCarrierMessage,
) -> Result<()> {
    // Collect carrier chunks from transparent inputs, in order.
    let mut chunks: Vec<Chunk> = Vec::new();
    for (_idx, script_sig) in &tx.inputs {
        if let Some((index, data)) = zalkanes_carrier::chunk_from_script_sig(script_sig) {
            chunks.push(Chunk { index, data });
        }
    }

    let expected_hash = CodeHash(call.input_hash);
    match zalkanes_carrier::reconstruct(
        &chunks,
        call.carrier_count,
        call.input_length,
        &expected_hash,
    ) {
        Ok(calldata) => execute_call(
            store,
            commit,
            executions,
            tx,
            limits,
            tx_fuel_used,
            call.contract_id,
            call.opcode,
            calldata,
        ),
        Err(e) => {
            debug!(txid = %tx.txid, error = %e, "calldata reconstruction failed; rejecting carrier call");
            Ok(())
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn execute_call(
    store: &mut dyn StateStore,
    commit: &mut BlockCommit,
    executions: &mut Vec<Execution>,
    tx: &ParsedTransaction,
    limits: &mut BlockLimits,
    tx_fuel_used: &mut u64,
    contract_id: ContractId,
    opcode: u16,
    input: Vec<u8>,
) -> Result<()> {
    // Enforce per-tx and per-block fuel budgets. A call gets at most the
    // minimum of the per-call cap and the remaining tx/block budget.
    let remaining_tx = MAX_FUEL_PER_ZCASH_TX.saturating_sub(*tx_fuel_used);
    let remaining_block = MAX_FUEL_PER_ZCASH_BLOCK.saturating_sub(limits.fuel_used);
    let fuel_limit = MAX_FUEL_PER_CALL.min(remaining_tx).min(remaining_block);
    if fuel_limit == 0 {
        warn!(txid = %tx.txid, "fuel budget exhausted; skipping call");
        return Ok(());
    }

    let ctx = CallContext {
        contract_id,
        caller: None,
        txid: tx.txid,
        block_height: 0, // patched below; CallContext needs height — see note
        opcode,
        input,
        fuel_limit,
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

    // Account fuel into the tx and block budgets.
    *tx_fuel_used = tx_fuel_used.saturating_add(fuel_used);
    limits.fuel_used = limits.fuel_used.saturating_add(fuel_used);

    for w in writes {
        apply_write(commit, w);
    }

    // Compute post-call root for the execution record.
    let root_after = compute_root_after(store, commit);

    executions.push(Execution {
        txid: tx.txid,
        contract_id,
        opcode,
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
        debug!(contract = %contract_id, fuel_used, "call succeeded");
    } else {
        debug!(contract = %contract_id, "call failed");
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
