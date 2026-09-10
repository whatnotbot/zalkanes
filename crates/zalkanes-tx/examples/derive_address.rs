//! Derive transparent funding addresses for a signing key across networks.
//!
//! Used to prepare testnet funding: generate a random 32-byte secret key,
//! derive its testnet transparent (P2PKH) address, and request funds from a
//! faucet to that address. The secret key is passed on the command line and is
//! never written to disk or committed.
//!
//! ```text
//! cargo run --example derive_address -- <64-char-hex-secret-key>
//! ```

use anyhow::Result;
use zalkanes_tx::SigningKey;
use zcash_protocol::consensus::NetworkType;

fn main() -> Result<()> {
    let hex = std::env::args()
        .nth(1)
        .expect("usage: derive_address <64-char-hex-secret-key>");
    let bytes = hex::decode(&hex).map_err(|e| anyhow::anyhow!("decode key: {e}"))?;
    let arr: [u8; 32] = bytes
        .try_into()
        .map_err(|_| anyhow::anyhow!("secret key must be 32 bytes"))?;
    let key = SigningKey::from_secret_bytes(arr)?;
    let pubkey = key.compressed_pubkey();

    for (name, nt) in [
        ("testnet", NetworkType::Test),
        ("regtest", NetworkType::Regtest),
        ("mainnet", NetworkType::Main),
    ] {
        let addr = key.p2pkh_address().to_zcash_address(nt).to_string();
        println!("{name}_p2pkh: {addr}");
    }
    println!("pubkey: {}", hex::encode(pubkey));
    Ok(())
}
