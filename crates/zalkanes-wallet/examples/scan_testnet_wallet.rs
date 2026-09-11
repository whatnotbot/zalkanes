//! Sync the testnet shielded wallet against our Zebra and report the scanned
//! identity, balance, and discovered note (no secrets).

#![forbid(unsafe_code)]

use anyhow::{anyhow, Result};
use secrecy::{ExposeSecret, SecretVec};
use zalkanes_wallet::{
    funding::TipSource, ShieldedWallet, SqliteShieldedWallet, ZebraCanonicalChainSource,
};
use zcash_protocol::consensus::Network;

fn main() -> Result<()> {
    let rpc_url = std::env::var("ZALKANES_TESTNET_RPC_URL")
        .unwrap_or_else(|_| "http://altaria.proxy.rlwy.net:14833".to_string());
    let wallet_dir = std::env::var("ZALKANES_WALLET_DIR").unwrap_or_else(|_| {
        dirs_home()
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join(".zalkanes/wallet/testnet")
            .to_string_lossy()
            .to_string()
    });
    let wallet_dir = std::path::PathBuf::from(&wallet_dir);
    let seed_path = wallet_dir.join("seed");
    let db_path = wallet_dir.join("wallet.sqlite");

    let hex = std::fs::read_to_string(&seed_path)
        .map_err(|e| anyhow!("read seed: {e}"))?
        .trim()
        .to_string();
    let seed = SecretVec::new(hex::decode(&hex).map_err(|e| anyhow!("decode seed: {e}"))?);

    let chain_source = ZebraCanonicalChainSource::new(rpc_url, Network::TestNetwork)?;
    let target = chain_source.canonical_tip()?;
    println!(
        "canonical tip: height={} hash={}",
        target.height,
        hex::encode(target.hash)
    );

    let wallet = SqliteShieldedWallet::reopen(&db_path, Network::TestNetwork, seed)?;
    println!("UA:       {}", wallet.unified_address()?);
    println!("birthday: {}", wallet.birthday_height()?);
    println!("pre-scan balance: {} zat", wallet.balance()?);

    wallet.scan_to_tip(&chain_source)?;

    let status = wallet.sync_status(target)?;
    println!(
        "scanned:  height={} hash={} synced={}",
        status.wallet_scan_height,
        hex::encode(status.wallet_scan_hash),
        status.synced
    );
    println!(
        "balance:  {} zat ({} ZEC)",
        wallet.balance()?,
        wallet.balance()? as f64 / 1e8
    );
    Ok(())
}

fn dirs_home() -> Option<std::path::PathBuf> {
    std::env::var_os("HOME").map(std::path::PathBuf::from)
}
