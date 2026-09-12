//! zalkanes CLI binary

#![forbid(unsafe_code)]

mod rpc;
mod wallet_cmd;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use std::sync::{Arc, RwLock};
use zalkanes_chain::{ChainSource, RpcChainSource};
use zalkanes_core::types::{BlockHash, Network};
use zalkanes_indexer::{process_zcash_block, IndexerConfig};
use zalkanes_rpc::{RpcHandler, SharedState};
use zalkanes_state::{BlockCommit, RocksState, StateStore};

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
    /// Wallet: custody, status, and lock state
    Wallet {
        #[command(subcommand)]
        cmd: WalletCmd,
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
enum WalletCmd {
    /// Create a new wallet + encrypted keystore (prompts for a passphrase)
    Create,
    /// Restore a wallet from an existing keystore at an explicit birthday
    Restore {
        /// First block height to scan (the wallet never guesses this)
        #[arg(long)]
        birthday: u32,
    },
    /// Print the wallet's unified address
    Address,
    /// Print the wallet's spendable balance
    Balance,
    /// Print wallet status, lock state, and sync position
    Status,
    /// Scan the wallet forward to the canonical tip (works while LOCKED)
    Scan,
    /// Arm spending for this wallet (verifies the passphrase; stores no secret)
    Unlock {
        /// Session lifetime in seconds (default 900)
        #[arg(long)]
        ttl: Option<u64>,
    },
    /// Disarm spending immediately
    Lock,
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
        /// Funding pool: transparent | shielded | auto
        #[arg(long, value_enum, default_value_t = wallet_cmd::FundingMode::Transparent)]
        funding: wallet_cmd::FundingMode,
        /// Plan and display only: no proof, no signature, no broadcast
        #[arg(long)]
        dry_run: bool,
        /// Skip the interactive confirmation
        #[arg(long)]
        yes: bool,
        /// Required for any money-moving mainnet command (--yes does not imply it)
        #[arg(long)]
        confirm_mainnet: bool,
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
        /// Funding pool: transparent | shielded | auto
        #[arg(long, value_enum, default_value_t = wallet_cmd::FundingMode::Transparent)]
        funding: wallet_cmd::FundingMode,
        /// Plan and display only: no proof, no signature, no broadcast
        #[arg(long)]
        dry_run: bool,
        /// Skip the interactive confirmation
        #[arg(long)]
        yes: bool,
        /// Required for any money-moving mainnet command (--yes does not imply it)
        #[arg(long)]
        confirm_mainnet: bool,
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

    // Unix tools must not panic when their stdout is closed early (e.g.
    // `zalkanes wallet status | head -1`, or `| grep -q`). Rust's println!
    // panics on EPIPE; translate that into the conventional quiet exit.
    // (Done with a panic hook rather than resetting SIGPIPE, because this
    // crate forbids unsafe code.)
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let msg = info.to_string();
        if msg.contains("Broken pipe") || msg.contains("failed printing to stdout") {
            std::process::exit(0);
        }
        default_hook(info);
    }));

    let cli = Cli::parse();

    match cli.command {
        Commands::Node { cmd } => match cmd {
            NodeCmd::Status => node_status().await,
            NodeCmd::Serve { port, data_dir } => node_serve(port, data_dir).await,
        },

        Commands::Contract { cmd } => match cmd {
            ContractCmd::Build { manifest_path } => contract_build(&manifest_path),
            ContractCmd::Fund { blocks } => contract_fund(blocks).await,
            ContractCmd::Deploy {
                wasm_path,
                funding,
                dry_run,
                yes,
                confirm_mainnet,
                wait,
            } => {
                let opts = spend_options(funding, dry_run, yes, confirm_mainnet)?;
                tokio::task::block_in_place(|| contract_deploy(&wasm_path, opts, wait))
            }
            ContractCmd::Call {
                contract_id,
                opcode,
                input_hex,
                funding,
                dry_run,
                yes,
                confirm_mainnet,
                wait,
            } => {
                let opts = spend_options(funding, dry_run, yes, confirm_mainnet)?;
                tokio::task::block_in_place(|| {
                    contract_call(&contract_id, opcode, &input_hex, opts, wait)
                })
            }
            ContractCmd::View {
                contract_id,
                opcode,
                input_hex,
                rpc_url,
            } => contract_view(&rpc_url, &contract_id, opcode, &input_hex).await,
        },

        Commands::Wallet { cmd } => {
            let network = network_from_env()?;
            let zebra = zcash_rpc_url()?;
            match cmd {
                WalletCmd::Create => wallet_cmd::wallet_create(network, &zebra),
                WalletCmd::Restore { birthday } => {
                    wallet_cmd::wallet_restore(network, &zebra, birthday)
                }
                WalletCmd::Address => wallet_cmd::wallet_address(network),
                WalletCmd::Balance => wallet_cmd::wallet_balance(network),
                WalletCmd::Status => wallet_cmd::wallet_status(network, &zebra),
                WalletCmd::Scan => wallet_cmd::wallet_scan(network, &zebra),
                WalletCmd::Unlock { ttl } => wallet_cmd::wallet_unlock(network, ttl),
                WalletCmd::Lock => wallet_cmd::wallet_lock(network),
            }
        }

        Commands::StateRoot { height } => state_root(height).await,

        Commands::Trace { txid } => {
            println!("Tracing txid: {txid}");
            println!("(Connect to a running Zalkanes node with --rpc-url)");
            Ok(())
        }
    }
}

/// Assemble the shared options every money-moving command takes.
fn spend_options(
    funding: wallet_cmd::FundingMode,
    dry_run: bool,
    yes: bool,
    confirm_mainnet: bool,
) -> Result<wallet_cmd::SpendOptions> {
    let network = network_from_env()?;
    // Mainnet confirmation is checked BEFORE any configuration/IO, so the
    // guard cannot be bypassed by an unrelated setup failure.
    if network == Network::Mainnet && !confirm_mainnet && !dry_run {
        bail!(
            "refusing a mainnet money-moving command without --confirm-mainnet \
             (--yes does NOT imply mainnet confirmation)"
        );
    }
    Ok(wallet_cmd::SpendOptions {
        network,
        zebra_url: zcash_rpc_url()?,
        funding,
        dry_run,
        assume_yes: yes,
        confirm_mainnet,
    })
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

/// Read `ZALKANES_REGTEST_ACTIVATION_HEIGHT`, which exists so regtest boundary
/// tests can put blocks *below* the activation height (the manifest pins
/// regtest activation at 1, leaving no room).
///
/// Setting it on any real network is a hard error rather than a silent no-op:
/// an operator who believes they can move mainnet's activation with an
/// environment variable must find out immediately.
fn regtest_activation_override(network: Network) -> Result<Option<u32>> {
    let Ok(raw) = std::env::var("ZALKANES_REGTEST_ACTIVATION_HEIGHT") else {
        return Ok(None);
    };
    if network != Network::Regtest {
        anyhow::bail!(
            "ZALKANES_REGTEST_ACTIVATION_HEIGHT is set but the network is {network:?}. \
             Activation heights for real networks are fixed by the protocol manifest and \
             cannot be overridden. Unset this variable."
        );
    }
    let height: u32 = raw
        .trim()
        .parse()
        .map_err(|e| anyhow::anyhow!("ZALKANES_REGTEST_ACTIVATION_HEIGHT is not a height: {e}"))?;
    if height == 0 {
        anyhow::bail!("ZALKANES_REGTEST_ACTIVATION_HEIGHT must be >= 1");
    }
    tracing::warn!(
        height,
        "regtest activation height overridden (regtest only; manifest unchanged)"
    );
    Ok(Some(height))
}

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
        regtest_activation_override: regtest_activation_override(network)?,
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
                    std::thread::sleep(std::time::Duration::from_secs(5));
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

        // 2.5. Fast-forward below the activation height. Pre-activation blocks
        //    have no protocol effect (state is provably empty), so instead of
        //    fetching and deserializing each one we seek straight to
        //    `activation - 1` and record its canonical block hash. This is
        //    deterministic: the state root below activation is always the empty
        //    root, and a fresh reindex follows the identical path.
        if let Some(act) = config.activation_height() {
            if act > 1 {
                // Seek to the last below-activation block, clamped to the
                // current tip so a future activation height does not try to
                // fetch a block that does not exist yet.
                let target = (act - 1).min(tip.height);
                let below = {
                    let g = store.read().unwrap();
                    g.indexed_height().is_none_or(|h| h < target)
                };
                if below {
                    let hash = {
                        let s = source.clone();
                        tokio::task::spawn_blocking(move || s.block_hash(target)).await??
                    };
                    let mut g = store.write().unwrap();
                    g.commit_block(BlockCommit {
                        height: target,
                        zcash_block_hash: hash,
                        deploys: Vec::new(),
                        upserts: Vec::new(),
                        deletes: Vec::new(),
                    })?;
                    tracing::info!(
                        height = target,
                        hash = %hash,
                        "fast-forwarded below activation height"
                    );
                }
            }
        }

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

            // Below activation: skip the raw fetch + deserialization and commit
            // an empty block directly (the fast path in `process_zcash_block`
            // ignores `raw_block`). Reached only if a reorg rolls the indexer
            // below the fast-forward point.
            if let Some(act) = config.activation_height() {
                if next_height < act {
                    let result = {
                        let mut g = store.write().unwrap();
                        process_zcash_block(&mut **g, &config, next_height, BlockHash(hash.0), &[])
                    };
                    match result {
                        Ok(_) => continue,
                        Err(e) => {
                            tracing::error!(height = next_height, error = %e, "block processing failed");
                            break;
                        }
                    }
                }
            }

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
fn confirm_tx(rpc: &rpc::ZcashRpc, txid: &str, addr: &str, network: Network) -> Result<u64> {
    match network {
        Network::Regtest => {
            let blocks = rpc.generate_to_address(1, addr)?;
            let block_hash = blocks.into_iter().next().context("no block mined")?;
            let block_json = rpc.get_block(&block_hash, 1)?;
            Ok(block_json["height"].as_u64().unwrap_or(0))
        }
        Network::Testnet => loop {
            match rpc.get_raw_transaction(txid, 1) {
                Ok(tx) => {
                    if tx["confirmations"].as_u64().unwrap_or(0) >= 1 {
                        return Ok(tx["height"].as_u64().unwrap_or(0));
                    }
                }
                Err(e) => {
                    // Transient RPC/network errors must not abort the wait.
                    tracing::warn!("getrawtransaction transient error, retrying: {e:#}");
                }
            }
            std::thread::sleep(std::time::Duration::from_secs(15));
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

/// Deploy a contract through the hardened funding pipeline.
///
/// Two stages: PREPARE creates the public P2SH carrier outputs (fundable from
/// either pool), then DEPLOY spends those carriers, publishing the WASM
/// chunks in their scriptSigs. DEPLOY is ALWAYS a transparent
/// carrier-spending transaction — with `--funding shielded` only the PREPARE
/// stage is shielded. The privacy boundary is disclosed before signing.
fn contract_deploy(wasm_path: &str, opts: wallet_cmd::SpendOptions, wait: bool) -> Result<()> {
    let network = opts.network;
    let rpc = rpc::ZcashRpc::new(&opts.zebra_url)?;
    let key = signing_key()?;
    let addr = funding_address(&key, network);

    let wasm = std::fs::read(wasm_path).with_context(|| format!("reading {wasm_path}"))?;
    zalkanes_runtime::validate_module(&wasm)
        .map_err(|e| anyhow::anyhow!("WASM validation failed: {e}"))?;
    let chunks = zalkanes_tx::split_chunks(&wasm)?;
    let code_hash = zalkanes_core::types::CodeHash::of(&wasm);
    let deploy_op_return = zalkanes_protocol::encode_deploy(&zalkanes_protocol::DeployMessage {
        code_hash,
        code_length: wasm.len() as u32,
        chunk_count: chunks.len() as u8,
        output_index: 0,
    });
    println!(
        "wasm: {} bytes, {} chunk(s), code hash {}",
        wasm.len(),
        chunks.len(),
        hex::encode(code_hash.0)
    );

    // Size the carriers from the EXACT ZIP-317 deploy fee.
    let probe_outpoints: Vec<zalkanes_tx::OutPoint> = (0..chunks.len())
        .map(|i| zalkanes_tx::OutPoint::new([0xEE; 32], i as u32))
        .collect();
    let probe_values = vec![50_000u64; chunks.len()];
    let deploy_fee = zalkanes_tx::deploy_plan(
        &key,
        &probe_outpoints,
        &probe_values,
        &chunks,
        &deploy_op_return,
    )?
    .fee;
    let n = chunks.len() as u64;
    let carrier_each = (deploy_fee + 50_000).div_ceil(n);
    let carrier_values = vec![carrier_each; chunks.len()];
    let carrier_total: u64 = carrier_values.iter().sum();
    println!(
        "deploy fee: {deploy_fee} zat; carriers: {} x {carrier_each} zat",
        chunks.len()
    );

    // ── Stage 1: PREPARE (fundable from either pool) ────────────────────────
    let transparent_for_prepare =
        || -> Result<(zalkanes_tx::SigningKey, Vec<zalkanes_wallet::FundingUtxo>)> {
            let (outpoint, value) = obtain_funding_utxo(&rpc, &addr, network)?;
            Ok((
                key.clone(),
                vec![zalkanes_wallet::FundingUtxo { outpoint, value }],
            ))
        };
    println!("── PREPARE ──");
    let prepare_txid = match wallet_cmd::execute_request(
        &opts,
        zalkanes_wallet::TxRequest::Prepare {
            carrier_values: carrier_values.clone(),
        },
        carrier_total + 50_000,
        transparent_for_prepare,
    )? {
        Some(txid) => txid,
        None => return Ok(()), // --dry-run or declined
    };
    let prepare_hex = display_txid(prepare_txid);
    confirm_tx(&rpc, &prepare_hex, &addr, network)?;
    println!("PREPARE mined: {prepare_hex}");

    // ── Stage 2: DEPLOY (always transparent carrier spends) ─────────────────
    println!("── DEPLOY (transparent carrier spends) ──");
    let carrier_outpoints: Vec<zalkanes_tx::OutPoint> = (0..chunks.len())
        .map(|i| zalkanes_tx::OutPoint::new(prepare_txid, i as u32))
        .collect();
    let deploy_opts = wallet_cmd::SpendOptions {
        funding: wallet_cmd::FundingMode::Transparent,
        ..opts
    };
    let carriers_for_deploy = {
        let outpoints = carrier_outpoints.clone();
        let values = carrier_values.clone();
        let key = key.clone();
        move || -> Result<(zalkanes_tx::SigningKey, Vec<zalkanes_wallet::FundingUtxo>)> {
            Ok((
                key.clone(),
                outpoints
                    .iter()
                    .zip(&values)
                    .map(|(o, v)| zalkanes_wallet::FundingUtxo {
                        outpoint: o.clone(),
                        value: *v,
                    })
                    .collect(),
            ))
        }
    };
    let deploy_txid = match wallet_cmd::execute_request(
        &deploy_opts,
        zalkanes_wallet::TxRequest::Deploy {
            chunks,
            carrier_outpoints,
            carrier_values,
            op_return: deploy_op_return,
        },
        0,
        carriers_for_deploy,
    )? {
        Some(txid) => txid,
        None => return Ok(()),
    };
    let deploy_hex = display_txid(deploy_txid);
    let height = confirm_tx(&rpc, &deploy_hex, &addr, network)?;
    println!("DEPLOY mined at height {height}: {deploy_hex}");

    let contract_id = zalkanes_core::types::ContractId::derive(
        network,
        &zalkanes_core::types::TxId(deploy_txid),
        0,
        &code_hash,
    );
    println!("ContractId: {}", hex::encode(contract_id.0));

    if wait {
        wait_for_contract(&hex::encode(contract_id.0))?;
    }
    Ok(())
}

/// Call a contract through the hardened funding pipeline.
fn contract_call(
    contract_id: &str,
    opcode: u16,
    input_hex: &str,
    opts: wallet_cmd::SpendOptions,
    wait: bool,
) -> Result<()> {
    let network = opts.network;
    let rpc = rpc::ZcashRpc::new(&opts.zebra_url)?;
    let key = signing_key()?;
    let addr = funding_address(&key, network);

    let mut id = [0u8; 32];
    hex::decode_to_slice(contract_id, &mut id).context("contract id must be 32 hex bytes")?;
    let input = hex::decode(input_hex).context("input must be hex")?;
    let op_return = zalkanes_protocol::encode_call(&zalkanes_protocol::CallMessage {
        contract_id: zalkanes_core::types::ContractId(id),
        opcode,
        input,
    });
    println!("ZALK payload: {}", hex::encode(&op_return));

    let transparent = || -> Result<(zalkanes_tx::SigningKey, Vec<zalkanes_wallet::FundingUtxo>)> {
        let (outpoint, value) = obtain_funding_utxo(&rpc, &addr, network)?;
        Ok((
            key.clone(),
            vec![zalkanes_wallet::FundingUtxo { outpoint, value }],
        ))
    };
    let txid = match wallet_cmd::execute_request(
        &opts,
        zalkanes_wallet::TxRequest::Call { op_return },
        50_000,
        transparent,
    )? {
        Some(txid) => txid,
        None => return Ok(()),
    };
    let txid_hex = display_txid(txid);
    let height = confirm_tx(&rpc, &txid_hex, &addr, network)?;
    println!("CALL mined at height {height}: {txid_hex}");

    if wait {
        wait_for_execution(&txid_hex)?;
    }
    Ok(())
}

/// Poll the Zalkanes node until `contract_id` is indexed.
fn wait_for_contract(contract_id: &str) -> Result<()> {
    let url = std::env::var("ZALKANES_URL").unwrap_or_else(|_| "http://127.0.0.1:3030".into());
    let client = reqwest::blocking::Client::new();
    for _ in 0..120 {
        let resp: serde_json::Value = client
            .post(&url)
            .json(&serde_json::json!({
                "jsonrpc":"2.0","id":1,"method":"zalkanes_getContract","params":[contract_id]
            }))
            .send()?
            .json()?;
        if resp.get("result").filter(|r| !r.is_null()).is_some() {
            println!("indexed: {}", resp["result"]);
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_secs(5));
    }
    bail!("contract {contract_id} was not indexed within 10 minutes")
}

/// Poll the Zalkanes node until `txid`'s execution record is indexed.
fn wait_for_execution(txid: &str) -> Result<()> {
    let url = std::env::var("ZALKANES_URL").unwrap_or_else(|_| "http://127.0.0.1:3030".into());
    let client = reqwest::blocking::Client::new();
    for _ in 0..120 {
        let resp: serde_json::Value = client
            .post(&url)
            .json(&serde_json::json!({
                "jsonrpc":"2.0","id":1,"method":"zalkanes_getExecution","params":[txid]
            }))
            .send()?
            .json()?;
        if resp.get("result").filter(|r| !r.is_null()).is_some() {
            println!("execution: {}", resp["result"]);
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_secs(5));
    }
    bail!("execution for {txid} was not indexed within 10 minutes")
}

/// Display (byte-reversed) txid, as Zcash RPCs and explorers show it.
fn display_txid(internal: [u8; 32]) -> String {
    let mut d = internal;
    d.reverse();
    hex::encode(d)
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
