//! zalkanes CLI binary

#![forbid(unsafe_code)]

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use zalkanes_chain::{ChainSource, RpcChainSource};
use zalkanes_core::types::Network;
use zalkanes_rpc::RpcHandler;

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
    /// Show node sync status (one-shot)
    Status,
    /// Run the node: connect to a Zcash RPC upstream and serve JSON-RPC on $PORT
    Serve {
        /// Bind port (default 3030; override with $PORT on Railway)
        #[arg(long)]
        port: Option<u16>,
    },
}

#[derive(Subcommand)]
enum ContractCmd {
    /// Scaffold a new contract project
    New { name: String },
    /// Build the contract in the current directory
    Build,
    /// Deploy a compiled WASM contract
    Deploy { wasm_path: String },
    /// Call a contract method
    Call {
        contract_id: String,
        opcode: u16,
        #[arg(default_value = "")]
        input_hex: String,
    },
    /// View (read-only call) a contract method
    View {
        contract_id: String,
        opcode: u16,
        #[arg(default_value = "")]
        input_hex: String,
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
            NodeCmd::Serve { port } => node_serve(port).await,
        },

        Commands::Contract { cmd } => match cmd {
            ContractCmd::New { name } => {
                println!("Scaffolding new contract: {name}");
                println!("  → contracts/{name}/Cargo.toml");
                println!("  → contracts/{name}/src/lib.rs");
                println!("See contracts/counter/ for a complete example.");
                Ok(())
            }
            ContractCmd::Build => {
                println!("Building contract for wasm32-unknown-unknown...");
                println!("Run: cargo build --release --target wasm32-unknown-unknown");
                println!("Reproducible flags: RUSTFLAGS=-C link-arg=-s SOURCE_DATE_EPOCH=0");
                Ok(())
            }
            ContractCmd::Deploy { wasm_path } => {
                println!("Deploying: {wasm_path}");
                println!("(Connect to a running Zalkanes node with --rpc-url)");
                Ok(())
            }
            ContractCmd::Call {
                contract_id,
                opcode,
                input_hex,
            } => {
                println!("Calling contract {contract_id} opcode={opcode} input={input_hex}");
                Ok(())
            }
            ContractCmd::View {
                contract_id,
                opcode,
                input_hex,
            } => {
                println!("Viewing contract {contract_id} opcode={opcode} input={input_hex}");
                Ok(())
            }
        },

        Commands::StateRoot { height } => match height {
            Some(h) => {
                println!("State root at height {h}: (connect to a running node)");
                Ok(())
            }
            None => {
                println!("Current state root: (connect to a running node)");
                Ok(())
            }
        },

        Commands::Trace { txid } => {
            println!("Tracing txid: {txid}");
            println!("(Connect to a running Zalkanes node with --rpc-url)");
            Ok(())
        }
    }
}

/// Read the upstream chain source configuration from environment variables.
///
/// - `ZALKANES_NETWORK`      — `mainnet` | `testnet` | `regtest` (default: `regtest`)
/// - `ZALKANES_RPC_URL`      — upstream Zcash JSON-RPC endpoint (required for `serve`)
/// - `ZALKANES_RPC_API_KEY`  — optional `api-key` header (hosted providers)
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
        .context("ZALKANES_RPC_URL not set (e.g. https://zec.nownodes.io/<api_key>)")?;

    let api_key = std::env::var("ZALKANES_RPC_API_KEY").ok();

    Ok((network, rpc_url, api_key))
}

async fn node_status() -> Result<()> {
    let (network, rpc_url, api_key) = match chain_config() {
        Ok(c) => c,
        Err(e) => {
            println!("Zalkanes node status");
            println!("Protocol version: 0");
            println!("Mainnet activation: UNSET (pre-audit)");
            println!("Regtest activation: 1");
            println!("Config: {e}");
            return Ok(());
        }
    };

    let source = std::sync::Arc::new(build_source(network, &rpc_url, api_key.as_deref())?);

    // The chain source is blocking; run validation off the async runtime.
    {
        let s = source.clone();
        tokio::task::spawn_blocking(move || s.validate()).await??;
    }
    let tip = {
        let s = source.clone();
        tokio::task::spawn_blocking(move || s.tip()).await??
    };

    println!("Zalkanes node status");
    println!("  Network:      {}", network.zebra_name());
    println!("  Chain tip:    height={} hash={}", tip.height, tip.hash);
    println!("  Protocol:     v0");
    println!("  Mainnet act.: UNSET (pre-audit)");
    println!("  Regtest act.: 1");
    Ok(())
}

async fn node_serve(port: Option<u16>) -> Result<()> {
    let port = port
        .or_else(|| std::env::var("PORT").ok().and_then(|p| p.parse().ok()))
        .unwrap_or(3030);

    let (network, rpc_url, api_key) = chain_config()?;
    let source = build_source(network, &rpc_url, api_key.as_deref())?;

    // Wrap the blocking source in an Arc for sharing with the tip poller.
    let source = std::sync::Arc::new(source);

    let handler = RpcHandler::new(network);

    // Validate + fetch initial tip.
    {
        let source = source.clone();
        let tip = tokio::task::spawn_blocking(move || {
            source.validate()?;
            source.tip()
        })
        .await??;
        tracing::info!(
            network = network.zebra_name(),
            height = tip.height,
            hash = %tip.hash,
            "chain source validated"
        );
        *handler.chain_tip_handle().write().unwrap() = Some(tip);
    }

    // Background poller: refresh the chain tip every 30s.
    {
        let source = source.clone();
        let tip_handle = handler.chain_tip_handle();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
            loop {
                interval.tick().await;
                let source = source.clone();
                match tokio::task::spawn_blocking(move || source.tip()).await {
                    Ok(Ok(tip)) => {
                        *tip_handle.write().unwrap() = Some(tip);
                    }
                    Ok(Err(e)) => tracing::warn!("tip poll failed: {e}"),
                    Err(e) => tracing::warn!("tip poll task failed: {e}"),
                }
            }
        });
    }

    let addr = std::net::SocketAddr::from(([0, 0, 0, 0], port));
    tracing::info!(%addr, "Zalkanes JSON-RPC server listening");
    zalkanes_rpc::serve(addr, handler).await
}

fn build_source(network: Network, rpc_url: &str, api_key: Option<&str>) -> Result<RpcChainSource> {
    let mut builder = RpcChainSource::builder(rpc_url, network);
    if let Some(key) = api_key {
        builder = builder.api_key(key);
    }
    builder.build()
}
