//! zalkanes CLI binary

#![forbid(unsafe_code)]

use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "zalkanes", about = "Zalkanes — Zcash-native WASM smart contracts")]
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
    StateRoot {
        height: Option<u32>,
    },
    /// Trace a Zcash transaction through Zalkanes execution
    Trace {
        txid: String,
    },
}

#[derive(Subcommand)]
enum NodeCmd {
    /// Show node sync status
    Status,
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
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("zalkanes=info".parse().unwrap()),
        )
        .init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Node { cmd } => match cmd {
            NodeCmd::Status => {
                println!("Zalkanes node status");
                println!("Protocol version: 0");
                println!("Mainnet activation: UNSET (pre-audit)");
                println!("Regtest activation: 1");
                println!("Run with a Zebra regtest node for testing.");
            }
        },

        Commands::Contract { cmd } => match cmd {
            ContractCmd::New { name } => {
                println!("Scaffolding new contract: {name}");
                println!("  → contracts/{name}/Cargo.toml");
                println!("  → contracts/{name}/src/lib.rs");
                println!("See contracts/counter/ for a complete example.");
            }
            ContractCmd::Build => {
                println!("Building contract for wasm32-unknown-unknown...");
                println!("Run: cargo build --release --target wasm32-unknown-unknown");
                println!("Reproducible flags: RUSTFLAGS=-C link-arg=-s SOURCE_DATE_EPOCH=0");
            }
            ContractCmd::Deploy { wasm_path } => {
                println!("Deploying: {wasm_path}");
                println!("(Connect to a running Zalkanes node with --rpc-url)");
            }
            ContractCmd::Call { contract_id, opcode, input_hex } => {
                println!("Calling contract {contract_id} opcode={opcode} input={input_hex}");
            }
            ContractCmd::View { contract_id, opcode, input_hex } => {
                println!("Viewing contract {contract_id} opcode={opcode} input={input_hex}");
            }
        },

        Commands::StateRoot { height } => {
            match height {
                Some(h) => println!("State root at height {h}: (connect to a running node)"),
                None => println!("Current state root: (connect to a running node)"),
            }
        }

        Commands::Trace { txid } => {
            println!("Tracing txid: {txid}");
            println!("(Connect to a running Zalkanes node with --rpc-url)");
        }
    }

    Ok(())
}
