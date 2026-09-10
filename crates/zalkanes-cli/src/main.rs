//! zalkanes CLI binary

#![forbid(unsafe_code)]

mod rpc;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use std::sync::{Arc, RwLock};
use zalkanes_chain::{ChainSource, RpcChainSource};
use zalkanes_core::types::{BlockHash, Network};
use zalkanes_indexer::{process_zcash_block, IndexerConfig};
use zalkanes_rpc::{RpcHandler, SharedState};
use zalkanes_state::{RocksState, StateStore};

#[derive(Parser)]
#[command(
    name = "zalkanes",
    about = "Zalkanes — Zcash-native WASM smart contracts"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Node management
    Node {
        #[command(subcommand)]
        cmd: NodeCmd,
    },
    /// Contract tools
    Contract {
        #[command(subcommand)]
        cmd: ContractCmd,
    },
    /// Query state root at a given height
    StateRoot { height: Option<u32> },
    /// Trace a Zcash transaction through Zalkanes execution
    Trace { txid: String },
}

#[derive(Subcommand)]
enum NodeCmd {
    /// Show node sync status (reads the persistent database)
    Status,
    /// Run the node: index real Zcash blocks into persistent state and serve JSON-RPC
    Serve {
        /// Bind port (default 3030; override with $PORT on Railway)
        #[arg(long)]
        port: Option<u16>,
        /// Data directory for persistent state (default $ZALKANES_DATA_DIR or ./zalkanes-data)
        #[arg(long, env = "ZALKANES_DATA_DIR")]
        data_dir: Option<String>,
    },
}

#[derive(Subcommand)]
enum ContractCmd {
    /// Build a contract for wasm32-unknown-unknown
    Build {
        /// Path to the contract crate
        #[arg(long, default_value = "./contracts/counter")]
        manifest_path: String,
    },
    /// Mine blocks to the dev funding address (regtest)
    Fund {
        /// Number of blocks to mine (default 110 for coinbase maturity)
        #[arg(long, default_value = "110")]
        blocks: u32,
    },
    /// Deploy a compiled WASM contract via a real Zcash transaction
    Deploy {
        wasm_path: String,
        /// Wait for Zalkanes to index the deployment
        #[arg(long)]
        wait: bool,
    },
    /// Call a contract method via a real Zcash transaction
    Call {
        contract_id: String,
        opcode: u16,
        #[arg(default_value = "")]
        input_hex: String,
        /// Wait for Zalkanes to index the call
        #[arg(long)]
        wait: bool,
    },
    /// View (read-only call) a contract method against a live node
    View {
        contract_id: String,
        opcode: u16,
        #[arg(default_value = "")]
        input_hex: String,
        /// Zalkanes JSON-RPC URL (default $ZALKANES_URL or http://127.0.0.1:3030)
        #[arg(long, env = "ZALKANES_URL", default_value = "http://127.0.0.1:3030")]
        rpc_url: String,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "zalkanes=info".into()),
        )
        .init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Node { cmd } => match cmd {
            NodeCmd::Status => node_status().await,
            NodeCmd::Serve { port, data_dir } => node_serve(port, data_dir).await,
        },

        Commands::Contract { cmd } => match cmd {
            ContractCmd::Build { manifest_path } => contract_build(&manifest_path),
            ContractCmd::Fund { blocks } => contract_fund(blocks).await,
            ContractCmd::Deploy { wasm_path, wait } => contract_deploy(&wasm_path, wait).await,
            ContractCmd::Call {
                contract_id,
                opcode,
                input_hex,
                wait,
            } => contract_call(&contract_id, opcode, &input_hex, wait).await,
            ContractCmd::View {
                contract_id,
                opcode,
                input_hex,
                rpc_url,
            } => contract_view(&rpc_url, &contract_id, opcode, &input_hex).await,
        },

        Commands::StateRoot { height } => state_root(height).await,

        Commands::Trace { txid } => {
            println!("Tracing txid: {txid}");
            println!("(Connect to a running Zalkanes node with --rpc-url)");
            Ok(())
        }
    }
}

// ── Config ───────────────────────────────────────────────────────────────────

fn chain_config() -> Result<(Network, String, Option<String>)> {
    let network = network_from_env()?;

    let rpc_url = std::env::var("ZALKANES_RPC_URL")
        .context("ZALKANES_RPC_URL not set (e.g. http://127.0.0.1:18232 for Zebra)")?;

    let api_key = std::env::var("ZALKANES_RPC_API_KEY").ok();

    Ok((network, rpc_url, api_key))
}

/// Parse the Zalkanes network from `ZALKANES_NETWORK` (defaults to regtest).
fn network_from_env() -> Result<Network> {
    match std::env::var("ZALKANES_NETWORK")
        .unwrap_or_else(|_| "regtest".to_string())
        .as_str()
    {
        "mainnet" | "main" => Ok(Network::Mainnet),
        "testnet" | "test" => Ok(Network::Testnet),
        "regtest" => Ok(Network::Regtest),
        other => bail!("unknown ZALKANES_NETWORK: {other}"),
    }
}

fn build_source(network: Network, rpc_url: &str, api_key: Option<&str>) -> Result<RpcChainSource> {
    let mut builder = RpcChainSource::builder(rpc_url, network);
    if let Some(key) = api_key {
        builder = builder.api_key(key);
    }
    builder.build()
}

fn data_dir(data_dir: Option<String>) -> std::path::PathBuf {
    if let Some(d) = data_dir {
        return std::path::PathBuf::from(d);
    }
    std::env::var("ZALKANES_DATA_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from("./zalkanes-data"))
}

// ── node status ──────────────────────────────────────────────────────────────

async fn node_status() -> Result<()> {
    let dir = data_dir(None);
    let (network, _, _) = match chain_config() {
        Ok(c) => c,
        Err(e) => {
            println!("Config: {e}");
            return Ok(());
        }
    };

    if !dir.exists() {
        println!("No local state database at {}", dir.display());
        println!("Run `zalkanes node serve` first.");
        return Ok(());
    }

    let store = RocksState::open(&dir)?;
    let (indexed_height, block_hash, root) = {
        let s: &dyn StateStore = &store;
        (s.indexed_height(), s.indexed_block_hash(), s.compute_root())
    };

    println!("Zalkanes node status");
    println!("  Network:          {}", network.zebra_name());
    println!(
        "  Indexed height:   {}",
        indexed_height.map_or("none".into(), |h| h.to_string())
    );
    println!(
        "  Indexed block:    {}",
        block_hash.map_or("none".into(), |h| hex::encode(h.0))
    );
    println!("  State root:       {root}");
    println!("  Mainnet act.:     UNSET (pre-audit)");
    println!("  Regtest act.:     1");
    Ok(())
}

// ── node serve (real indexing loop) ──────────────────────────────────────────

async fn node_serve(port: Option<u16>, data_dir_opt: Option<String>) -> Result<()> {
    let port = port
        .or_else(|| std::env::var("PORT").ok().and_then(|p| p.parse().ok()))
        .unwrap_or(3030);

    let (network, rpc_url, api_key) = chain_config()?;
    let source = Arc::new(build_source(network, &rpc_url, api_key.as_deref())?);

    // Open the persistent state database.
    let dir = data_dir(data_dir_opt);
    let store = Arc::new(RwLock::new(
        Box::new(RocksState::open(&dir)?) as Box<dyn StateStore>
    ));

    // The RPC handler shares this exact store.
    let handler = RpcHandler::new(network, store.clone());
    let config = IndexerConfig {
        network,
        data_dir: dir.clone(),
    };

    // Validate upstream connection + fetch initial tip, retrying with backoff
    // so a brief upstream outage (or Zebra still starting) does not kill the
    // indexer. The process must stay alive and keep trying.
    {
        let s = source.clone();
        let mut attempts = 0u32;
        loop {
            attempts += 1;
            let s = s.clone();
            match tokio::task::spawn_blocking(move || {
                s.validate()?;
                s.tip()
            })
            .await
            {
                Ok(Ok(tip)) => {
                    tracing::info!(
                        network = network.zebra_name(),
                        height = tip.height,
                        hash = %tip.hash,
                        attempts,
                        "chain source connection verified"
                    );
                    *handler.chain_tip_handle().write().unwrap() = Some(tip);
                    break;
                }
                other => {
                    let msg = match other {
                        Ok(Ok(_)) => "unreachable".to_string(),
                        Ok(Err(e)) => format!("{e:#}"),
                        Err(e) => format!("{e}"),
                    };
                    tracing::warn!(attempts, error = %msg, "upstream not reachable; retrying");
                    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                }
            }
        }
    }

    // Spawn the indexing loop.
    {
        let source = source.clone();
        let store = store.clone();
        let handler = handler.clone();
        let config = config.clone();
        tokio::spawn(async move {
            if let Err(e) = indexing_loop(source, store, handler, config).await {
                tracing::error!("indexing loop failed: {e:#}");
            }
        });
    }

    // Spawn the health/readiness endpoint on a separate port.
    {
        let handler = handler.clone();
        let health_port = port.saturating_add(1);
        let health_addr = std::net::SocketAddr::from(([0, 0, 0, 0], health_port));
        tokio::spawn(async move {
            if let Err(e) = zalkanes_rpc::serve_health(health_addr, handler).await {
                tracing::error!("health server failed: {e:#}");
            }
        });
    }

    let addr = std::net::SocketAddr::from(([0, 0, 0, 0], port));
    tracing::info!(%addr, "Zalkanes JSON-RPC server listening");
    zalkanes_rpc::serve(addr, handler).await
}

/// The real indexing loop: pull canonical blocks from the chain source, parse
/// them, execute Zalkanes messages, and atomically persist state.
async fn indexing_loop(
    source: Arc<RpcChainSource>,
    store: SharedState,
    handler: RpcHandler,
    config: IndexerConfig,
) -> Result<()> {
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(3));
    loop {
        interval.tick().await;

        // 1. Refresh chain tip.
        let tip = {
            let s = source.clone();
            match tokio::task::spawn_blocking(move || s.tip()).await {
                Ok(Ok(t)) => t,
                Ok(Err(e)) => {
                    tracing::warn!("tip fetch failed: {e}");
                    continue;
                }
                Err(e) => {
                    tracing::warn!("tip task failed: {e}");
                    continue;
                }
            }
        };
        *handler.chain_tip_handle().write().unwrap() = Some(tip);

        // 2. Reorg detection: if our indexed tip no longer matches the canonical
        //    chain, roll back to the common ancestor.
        handle_reorg(&source, &store).await?;

        // 3. Determine next height to index.
        loop {
            let indexed = {
                let g = store.read().unwrap();
                g.indexed_height()
            };
            let next_height = indexed.map_or(1, |h| h + 1);

            if next_height > tip.height {
                break; // caught up
            }

            // 3. Fetch canonical block hash at height.
            let hash = {
                let s = source.clone();
                match tokio::task::spawn_blocking(move || s.block_hash(next_height)).await {
                    Ok(Ok(h)) => h,
                    Ok(Err(e)) => {
                        tracing::warn!(height = next_height, "block_hash failed: {e}");
                        break;
                    }
                    Err(e) => {
                        tracing::warn!(height = next_height, "block_hash task failed: {e}");
                        break;
                    }
                }
            };

            // 4. Fetch raw block.
            let raw = {
                let s = source.clone();
                match tokio::task::spawn_blocking(move || s.raw_block(&hash)).await {
                    Ok(Ok(r)) => r,
                    Ok(Err(e)) => {
                        tracing::warn!(height = next_height, "raw_block failed: {e}");
                        break;
                    }
                    Err(e) => {
                        tracing::warn!(height = next_height, "raw_block task failed: {e}");
                        break;
                    }
                }
            };

            // 5. Process against persistent state.
            let result = {
                let mut g = store.write().unwrap();
                process_zcash_block(&mut **g, &config, next_height, BlockHash(hash.0), &raw)
            };
            match result {
                Ok(exec) => {
                    tracing::info!(
                        height = next_height,
                        executions = exec.executions.len(),
                        deployed = exec.deployed.len(),
                        root = %exec.state_root_after,
                        "indexed block"
                    );
                }
                Err(e) => {
                    // A deserialization/processing failure must not wedge the
                    // loop; log and continue. Reorgs are handled by rollback
                    // logic in process_zcash_block's commit path.
                    tracing::error!(height = next_height, error = %e, "block processing failed");
                    break;
                }
            }
        }
    }
}

/// Detect a reorg: compare our indexed block hash at each height against the
/// canonical chain's hash, rolling back until they agree.
async fn handle_reorg(source: &Arc<RpcChainSource>, store: &SharedState) -> Result<()> {
    loop {
        let (indexed_height, indexed_hash) = {
            let g = store.read().unwrap();
            (g.indexed_height(), g.indexed_block_hash())
        };

        let Some(height) = indexed_height else { break };
        let Some(our_hash) = indexed_hash else { break };

        let canonical_hash = {
            let s = source.clone();
            tokio::task::spawn_blocking(move || s.block_hash(height)).await??
        };

        if canonical_hash.0 == our_hash.0 {
            break; // in agreement — no reorg at this height
        }

        tracing::warn!(
            height,
            our = %hex::encode(our_hash.0),
            canonical = %hex::encode(canonical_hash.0),
            "reorg detected; rolling back"
        );

        {
            let mut g = store.write().unwrap();
            g.rollback_to(height - 1)?;
        }
    }
    Ok(())
}

// ── contract build ───────────────────────────────────────────────────────────

fn contract_build(manifest_path: &str) -> Result<()> {
    use std::process::Command;
    let status = Command::new("cargo")
        .args([
            "build",
            "--release",
            "--target",
            "wasm32-unknown-unknown",
            "--manifest-path",
            manifest_path,
        ])
        .env("RUSTFLAGS", "-C link-arg=-s")
        .env("SOURCE_DATE_EPOCH", "0")
        .status()
        .context("cargo build failed")?;
    if !status.success() {
        bail!("cargo build exited with {status}");
    }
    println!("Built {manifest_path} for wasm32-unknown-unknown");
    Ok(())
}

// ── contract deploy / call / view / fund ─────────────────────────────────────
//
// deploy/call broadcast REAL signed Zcash transactions against our Zebra node.
// The blockchain is the only state-mutating path.

/// Zebra RPC URL for broadcast/funding (env `ZALKANES_ZCASH_RPC_URL`).
fn zcash_rpc_url() -> Result<String> {
    std::env::var("ZALKANES_ZCASH_RPC_URL").context("ZALKANES_ZCASH_RPC_URL not set")
}

/// The signing key (env `ZALKANES_SIGNING_KEY` as 64-char hex, or a dev default).
fn signing_key() -> Result<zalkanes_tx::SigningKey> {
    match std::env::var("ZALKANES_SIGNING_KEY") {
        Ok(hex) => {
            let bytes = hex::decode(hex).context("decode ZALKANES_SIGNING_KEY hex")?;
            let arr: [u8; 32] = bytes
                .try_into()
                .map_err(|_| anyhow::anyhow!("signing key must be 32 bytes"))?;
            zalkanes_tx::SigningKey::from_secret_bytes(arr)
        }
        Err(_) => {
            eprintln!("WARNING: using deterministic dev signing key (regtest only)");
            Ok(zalkanes_tx::SigningKey::dev_key())
        }
    }
}

fn funding_address(key: &zalkanes_tx::SigningKey, network: Network) -> String {
    use zcash_protocol::consensus::NetworkType;
    let network_type = match network {
        Network::Mainnet => NetworkType::Main,
        Network::Testnet => NetworkType::Test,
        Network::Regtest => NetworkType::Regtest,
    };
    key.p2pkh_address()
        .to_zcash_address(network_type)
        .to_string()
}

/// The consensus branch ID for signing V4 transparent transactions on `network`.
fn network_branch_id(network: Network) -> Result<zcash_protocol::consensus::BranchId> {
    zalkanes_tx::branch_id_for_network(network)
}

/// Obtain a spendable funding UTXO for `addr`.
///
/// - Regtest: mine 110 blocks (coinbase maturity) and scan for a mature coinbase
///   output paying `addr`.
/// - Testnet: read `ZALKANES_FUNDING_TXID` + `ZALKANES_FUNDING_VOUT` and look the
///   output amount up from the node (funds arrive from an external faucet).
fn obtain_funding_utxo(
    rpc: &rpc::ZcashRpc,
    addr: &str,
    network: Network,
) -> Result<(zcash_transparent::bundle::OutPoint, u64)> {
    match network {
        Network::Regtest => {
            let blocks = rpc.generate_to_address(110, addr)?;
            rpc::find_mature_utxo(rpc, addr, &blocks)
        }
        Network::Testnet => {
            let txid = std::env::var("ZALKANES_FUNDING_TXID")
                .context("ZALKANES_FUNDING_TXID not set (testnet faucet txid)")?;
            let vout = std::env::var("ZALKANES_FUNDING_VOUT")
                .unwrap_or_else(|_| "0".to_string())
                .parse::<u32>()
                .context("ZALKANES_FUNDING_VOUT must be a vout index")?;
            let tx = rpc.get_raw_transaction(&txid, 1)?;
            let vouts = tx["vout"]
                .as_array()
                .context("funding tx has no vout array")?;
            let entry = vouts
                .get(vout as usize)
                .with_context(|| format!("funding tx has no vout {vout}"))?;
            let value = entry["valueZat"]
                .as_u64()
                .context("funding vout missing valueZat")?;
            let pays_us = entry["scriptPubKey"]["addresses"]
                .as_array()
                .map(|a| a.iter().any(|x| x.as_str() == Some(addr)))
                .unwrap_or(false);
            if !pays_us {
                bail!("funding tx vout {vout} does not pay {addr}");
            }
            let outpoint =
                zcash_transparent::bundle::OutPoint::new(rpc::rpc_txid_to_internal(&txid)?, vout);
            Ok((outpoint, value))
        }
        Network::Mainnet => bail!("mainnet activation is not set (pre-audit)"),
    }
}

/// Get `txid` mined and return its block height.
///
/// - Regtest: mine one block (internal miner).
/// - Testnet: poll `getrawtransaction` until the tx has ≥1 confirmation.
async fn confirm_tx(rpc: &rpc::ZcashRpc, txid: &str, addr: &str, network: Network) -> Result<u64> {
    match network {
        Network::Regtest => {
            let blocks = rpc.generate_to_address(1, addr)?;
            let block_hash = blocks.into_iter().next().context("no block mined")?;
            let block_json = rpc.get_block(&block_hash, 1)?;
            Ok(block_json["height"].as_u64().unwrap_or(0))
        }
        Network::Testnet => loop {
            let tx = rpc.get_raw_transaction(txid, 1)?;
            if tx["confirmations"].as_u64().unwrap_or(0) >= 1 {
                return Ok(tx["height"].as_u64().unwrap_or(0));
            }
            tokio::time::sleep(std::time::Duration::from_secs(15)).await;
        },
        Network::Mainnet => bail!("mainnet activation is not set (pre-audit)"),
    }
}

async fn contract_fund(blocks: u32) -> Result<()> {
    let network = network_from_env()?;
    if network != Network::Regtest {
        bail!(
            "contract fund only mines blocks on regtest; on {} obtain funds from a faucet",
            network.zebra_name()
        );
    }
    let rpc = rpc::ZcashRpc::new(&zcash_rpc_url()?)?;
    let key = signing_key()?;
    let addr = funding_address(&key, network);
    let mined = rpc.generate_to_address(blocks, &addr)?;
    println!("mined {blocks} blocks to {addr}");
    println!(
        "latest block: {}",
        mined.last().map(String::as_str).unwrap_or("")
    );
    Ok(())
}

async fn contract_deploy(wasm_path: &str, wait: bool) -> Result<()> {
    let network = network_from_env()?;
    let branch_id = network_branch_id(network)?;
    let rpc = rpc::ZcashRpc::new(&zcash_rpc_url()?)?;
    let key = signing_key()?;
    let addr = funding_address(&key, network);

    // Read + validate the WASM.
    let wasm = std::fs::read(wasm_path).with_context(|| format!("read {wasm_path}"))?;
    zalkanes_runtime::validate_module(&wasm)
        .map_err(|e| anyhow::anyhow!("WASM validation: {e}"))?;

    let code_hash = zalkanes_core::types::CodeHash::of(&wasm);
    println!("code_hash: {}", code_hash.as_hex());

    // Split into chunks and compute carrier counts.
    let chunks = zalkanes_tx::split_chunks(&wasm)?;
    let chunk_count = chunks.len() as u8;
    println!("chunks: {chunk_count}");

    // PREPARE: obtain a funding UTXO and create `chunk_count` carrier UTXOs.
    let (funding_outpoint, funding_value) = obtain_funding_utxo(&rpc, &addr, network)?;
    println!(
        "funding UTXO: {} zat (outpoint vout {})",
        funding_value,
        funding_outpoint.n()
    );

    // Each carrier UTXO needs enough value to later pay its share of the DEPLOY
    // fee; fund each with 1_000_000 zatoshi.
    let carrier_value = 1_000_000u64;
    let carrier_values = vec![carrier_value; chunk_count as usize];
    let prepare = zalkanes_tx::build_prepare(
        &key,
        &[(funding_outpoint, funding_value)],
        &carrier_values,
        branch_id,
    )?;
    let prepare_txid = prepare.txid_hex();
    let accepted = rpc.send_raw_transaction(&hex::encode(&prepare.bytes))?;
    if accepted != prepare_txid {
        bail!("PREPARE txid mismatch: sent {accepted} != expected {prepare_txid}");
    }
    println!("prepare_txid: {prepare_txid}");

    let prepare_height = confirm_tx(&rpc, &prepare_txid, &addr, network).await?;
    println!("prepare_block_height: {prepare_height}");

    // Build the DEPLOY OP_RETURN message.
    let deploy_msg = zalkanes_protocol::DeployMessage {
        code_hash,
        code_length: wasm.len() as u32,
        chunk_count,
        output_index: 0,
    };
    let op_return = zalkanes_protocol::encode_deploy(&deploy_msg);

    // Carrier outpoints are outputs 0..chunk_count-1 of the PREPARE tx.
    let mut carrier_outpoints = Vec::with_capacity(chunk_count as usize);
    for i in 0..chunk_count {
        carrier_outpoints.push(zcash_transparent::bundle::OutPoint::new(
            rpc::rpc_txid_to_internal(&prepare_txid)?,
            i as u32,
        ));
    }

    let deploy = zalkanes_tx::build_deploy(
        &key,
        &carrier_outpoints,
        &carrier_values,
        &chunks,
        &op_return,
        branch_id,
    )?;
    let deploy_txid = deploy.txid_hex();
    let accepted = rpc.send_raw_transaction(&hex::encode(&deploy.bytes))?;
    if accepted != deploy_txid {
        bail!("DEPLOY txid mismatch: sent {accepted} != expected {deploy_txid}");
    }
    println!("deploy_txid: {deploy_txid}");

    let deploy_height = confirm_tx(&rpc, &deploy_txid, &addr, network).await?;
    println!("deploy_block_height: {deploy_height}");

    // Compute the deterministic ContractId (must match the indexer's).
    let txid_internal = rpc::rpc_txid_to_internal(&deploy_txid)?;
    let contract_id = zalkanes_core::types::ContractId::derive(
        network,
        &zalkanes_core::types::TxId(txid_internal),
        0,
        &code_hash,
    );
    println!("contract_id: {}", contract_id.as_hex());

    if wait {
        wait_for_contract(&contract_id, &code_hash).await?;
        println!("indexed: true");
    }

    Ok(())
}

async fn contract_call(contract_id: &str, opcode: u16, input_hex: &str, wait: bool) -> Result<()> {
    let network = network_from_env()?;
    let branch_id = network_branch_id(network)?;
    let rpc = rpc::ZcashRpc::new(&zcash_rpc_url()?)?;
    let key = signing_key()?;
    let addr = funding_address(&key, network);

    let cid = hex::decode(contract_id).context("contract_id hex")?;
    if cid.len() != 32 {
        bail!("contract_id must be 32 bytes");
    }
    let mut cid_bytes = [0u8; 32];
    cid_bytes.copy_from_slice(&cid);

    let input = hex::decode(input_hex).context("input hex")?;

    // Build the CALL OP_RETURN message.
    let call_msg = zalkanes_protocol::CallMessage {
        contract_id: zalkanes_core::types::ContractId(cid_bytes),
        opcode,
        input,
    };
    let op_return = zalkanes_protocol::encode_call(&call_msg);

    // Spend a fresh mature funding UTXO.
    let (funding_outpoint, funding_value) = obtain_funding_utxo(&rpc, &addr, network)?;

    let call =
        zalkanes_tx::build_call(&key, funding_outpoint, funding_value, &op_return, branch_id)?;
    let txid = call.txid_hex();
    let accepted = rpc.send_raw_transaction(&hex::encode(&call.bytes))?;
    if accepted != txid {
        bail!("CALL txid mismatch: sent {accepted} != expected {txid}");
    }
    println!("txid: {txid}");

    let height = confirm_tx(&rpc, &txid, &addr, network).await?;
    println!("block_height: {height}");

    if wait {
        wait_for_execution(&txid).await?;
    }

    Ok(())
}

/// Poll the Zalkanes RPC until the contract is indexed.
async fn wait_for_contract(
    contract_id: &zalkanes_core::types::ContractId,
    code_hash: &zalkanes_core::types::CodeHash,
) -> Result<()> {
    let url = std::env::var("ZALKANES_URL").unwrap_or_else(|_| "http://127.0.0.1:3030".to_string());
    let client = reqwest::Client::new();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
    loop {
        if std::time::Instant::now() > deadline {
            bail!("timed out waiting for contract index");
        }
        let body = serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "zalkanes_getContract",
            "params": [contract_id.as_hex()],
        });
        let resp = client
            .post(&url)
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .with_context(|| format!("RPC call to {url}"))?;
        let text = resp.text().await?;
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
            if let Some(result) = v.get("result") {
                if !result.is_null() {
                    if let Some(hash) = result.get("code_hash").and_then(|h| h.as_str()) {
                        if hash == code_hash.as_hex() {
                            return Ok(());
                        }
                    }
                }
            }
        }
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
    }
}

/// Poll the Zalkanes RPC until a CALL execution for `txid` is recorded.
async fn wait_for_execution(txid: &str) -> Result<()> {
    let url = std::env::var("ZALKANES_URL").unwrap_or_else(|_| "http://127.0.0.1:3030".to_string());
    let client = reqwest::Client::new();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
    loop {
        if std::time::Instant::now() > deadline {
            bail!("timed out waiting for CALL execution");
        }
        let body = serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "zalkanes_getExecution",
            "params": [txid],
        });
        let resp = client
            .post(&url)
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .with_context(|| format!("RPC call to {url}"))?;
        let text = resp.text().await?;
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
            if let Some(result) = v.get("result") {
                if !result.is_null() {
                    println!("execution: {result}");
                    return Ok(());
                }
            }
        }
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
    }
}

async fn contract_view(
    rpc_url: &str,
    contract_id: &str,
    opcode: u16,
    input_hex: &str,
) -> Result<()> {
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "zalkanes_view",
        "params": [contract_id, opcode, input_hex],
    });
    let client = reqwest::Client::new();
    let resp = client
        .post(rpc_url)
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await
        .with_context(|| format!("RPC call to {rpc_url} failed"))?;
    let text = resp.text().await?;
    println!("{text}");
    Ok(())
}

async fn state_root(height: Option<u32>) -> Result<()> {
    let dir = data_dir(None);
    let store = RocksState::open(&dir)?;
    let s: &dyn StateStore = &store;
    match height {
        Some(h) => match s.height_history().iter().find(|r| r.height == h) {
            Some(rec) => println!("{}", rec.state_root),
            None => println!("(no record at height {h})"),
        },
        None => println!("{}", s.compute_root()),
    }
    Ok(())
}
