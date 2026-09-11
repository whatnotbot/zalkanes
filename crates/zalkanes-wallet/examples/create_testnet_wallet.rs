//! Create a fresh testnet shielded wallet against our own Zebra and print ONLY
//! the non-secret acceptance material: unified address, birthday height, and
//! the expected shielded receiver pool. The seed is written to a mode-600 file
//! (never printed).

#![forbid(unsafe_code)]

use std::io::Write;

use anyhow::{anyhow, Result};
use rand_core::{OsRng, RngCore};
use secrecy::{ExposeSecret, SecretVec};
use zalkanes_core::consensus_params::ConsensusParams;
use zalkanes_wallet::{SqliteShieldedWallet, ZebraCanonicalChainSource};

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
    std::fs::create_dir_all(&wallet_dir).map_err(|e| anyhow!("create wallet dir: {e}"))?;

    let seed_path = wallet_dir.join("seed");
    let db_path = wallet_dir.join("wallet.sqlite");

    // Generate or load the seed (never printed).
    let seed: SecretVec<u8> = if seed_path.exists() {
        let hex = std::fs::read_to_string(&seed_path)
            .map_err(|e| anyhow!("read seed: {e}"))?
            .trim()
            .to_string();
        let bytes = hex::decode(&hex).map_err(|e| anyhow!("decode seed: {e}"))?;
        SecretVec::new(bytes)
    } else {
        let mut bytes = vec![0u8; 32];
        OsRng.fill_bytes(&mut bytes);
        let seed = SecretVec::new(bytes);
        let hex = hex::encode(seed.expose_secret());
        let mut f = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&seed_path)
            .map_err(|e| anyhow!("write seed: {e}"))?;
        f.write_all(hex.as_bytes())
            .map_err(|e| anyhow!("write seed bytes: {e}"))?;
        // chmod 600 on unix.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&seed_path, std::fs::Permissions::from_mode(0o600))
                .map_err(|e| anyhow!("chmod seed: {e}"))?;
        }
        seed
    };

    let chain_source = ZebraCanonicalChainSource::new(rpc_url, ConsensusParams::Test)?;
    let wallet =
        SqliteShieldedWallet::create_new(&db_path, ConsensusParams::Test, seed, &chain_source)?;

    println!("testnet unified address: {}", wallet.unified_address()?);
    println!("wallet birthday height:  {}", wallet.birthday_height()?);
    println!("expected shielded receiver pool: orchard+ironwood (report actual after funding)");
    Ok(())
}

fn dirs_home() -> Option<std::path::PathBuf> {
    std::env::var_os("HOME").map(std::path::PathBuf::from)
}
