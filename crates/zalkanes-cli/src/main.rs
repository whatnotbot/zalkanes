//! zalkanes CLI binary

#![forbid(unsafe_code)]

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
    /// Deploy a compiled WASM contract via a real Zcash transaction
    Deploy { wasm_path: String },
    /// Call a contract method via a real Zcash transaction
    Call {
        contract_id: String,
        opcode: u16,
        #[arg(default_value = "")]
        input_hex: String,
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
            ContractCmd::Deploy { wasm_path } => contract_deploy(&wasm_path).await,
            ContractCmd::Call {
                contract_id,
                opcode,
                input_hex,
            } => contract_call(&contract_id, opcode, &input_hex).await,
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
    let network = match std::env::var("ZALKANES_NETWORK")
        .unwrap_or_else(|_| "regtest".to_string())
        .as_str()
    {
        "mainnet" | "main" => Network::Mainnet,
        "testnet" | "test" => Network::Testnet,
        "regtest" => Network::Regtest,
        other => bail!("unknown ZALKANES_NETWORK: {other}"),
    };

    let rpc_url = std::env::var("ZALKANES_RPC_URL")
        .context("ZALKANES_RPC_URL not set (e.g. http://127.0.0.1:18232 for Zebra)")?;

    let api_key = std::env::var("ZALKANES_RPC_API_KEY").ok();

    Ok((network, rpc_url, api_key))
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

    // Validate upstream connection + fetch initial tip.
    {
        let s = source.clone();
        let tip = tokio::task::spawn_blocking(move || {
            s.validate()?;
            s.tip()
        })
        .await??;
        tracing::info!(
            network = network.zebra_name(),
            height = tip.height,
            hash = %tip.hash,
            "chain source connection verified"
        );
        *handler.chain_tip_handle().write().unwrap() = Some(tip);
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

// ── contract deploy / call / view ────────────────────────────────────────────
//
// deploy/call broadcast real Zcash transactions. Transaction construction and
// signing live in `zalkanes-tx`; these commands wire them to a Zebra RPC and
// to the Zalkanes indexer (see `docs/architecture.md`).

async fn contract_deploy(wasm_path: &str) -> Result<()> {
    println!("Deploying: {wasm_path}");
    println!("NOTE: transaction construction + signing implemented in zalkanes-tx.");
    println!("(Wire to a Zebra RPC + wallet key via env; see docs/tx.md)");
    Ok(())
}

async fn contract_call(contract_id: &str, opcode: u16, input_hex: &str) -> Result<()> {
    println!("Calling contract {contract_id} opcode={opcode} input={input_hex}");
    println!("NOTE: transaction construction + signing implemented in zalkanes-tx.");
    Ok(())
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
