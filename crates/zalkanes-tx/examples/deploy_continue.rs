//! Testnet DEPLOY continuation: given a PREPARE txid, wait for it to confirm,
//! then build + broadcast the DEPLOY carrier transaction.
//!
//! Used to recover from a transient RPC disconnect mid-deploy without
//! double-spending the funding UTXO.
//!
//! Usage:
//! ```text
//! ZALKANES_ZCASH_RPC_URL=... ZALKANES_SIGNING_KEY=... \
//!   cargo run --example deploy_continue -- <wasm_path> <prepare_txid>
//! ```

use anyhow::{bail, Context, Result};
use serde_json::Value;
use zalkanes_tx::{build_deploy, split_chunks, SigningKey};
use zcash_protocol::consensus::NetworkType;
use zcash_transparent::bundle::OutPoint;

struct Rpc {
    url: String,
    client: reqwest::blocking::Client,
}

impl Rpc {
    fn new(url: &str) -> Self {
        Rpc {
            url: url.to_string(),
            client: reqwest::blocking::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .unwrap(),
        }
    }
    fn call(&self, method: &str, params: Value) -> Result<Value> {
        let body = serde_json::json!({"jsonrpc":"2.0","id":1,"method":method,"params":params});
        let resp = self
            .client
            .post(&self.url)
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .with_context(|| format!("rpc {method}"))?;
        let v: Value = resp.json().context("parse rpc response")?;
        if let Some(err) = v.get("error") {
            if !err.is_null() {
                bail!("rpc {method} error: {err}");
            }
        }
        Ok(v["result"].clone())
    }
    fn send_raw_transaction(&self, hex: &str) -> Result<String> {
        let v = self.call("sendrawtransaction", serde_json::json!([hex]))?;
        Ok(v.as_str().unwrap_or("").to_string())
    }
    fn get_raw_transaction(&self, txid: &str, verbosity: u8) -> Result<Value> {
        self.call("getrawtransaction", serde_json::json!([txid, verbosity]))
    }
    fn wait_confirmed(&self, txid: &str) -> Result<Value> {
        loop {
            let tx = self.get_raw_transaction(txid, 1)?;
            if tx["confirmations"].as_u64().unwrap_or(0) >= 1 {
                return Ok(tx);
            }
            std::thread::sleep(std::time::Duration::from_secs(10));
        }
    }
    fn tip_height(&self) -> Result<u32> {
        let v = self.call("getblockchaininfo", serde_json::json!([]))?;
        v["blocks"]
            .as_u64()
            .map(|h| h as u32)
            .context("getblockchaininfo missing blocks")
    }
}

fn rpc_txid_to_internal(display: &str) -> Result<[u8; 32]> {
    let bytes = hex::decode(display).context("decode txid hex")?;
    if bytes.len() != 32 {
        bail!("txid wrong length");
    }
    let mut internal = [0u8; 32];
    internal.copy_from_slice(&bytes);
    internal.reverse();
    Ok(internal)
}

fn main() -> Result<()> {
    let wasm_path = std::env::args()
        .nth(1)
        .context("usage: deploy_continue <wasm_path> <prepare_txid>")?;
    let prepare_txid = std::env::args()
        .nth(2)
        .context("usage: deploy_continue <wasm_path> <prepare_txid>")?;
    let rpc_url =
        std::env::var("ZALKANES_ZCASH_RPC_URL").context("ZALKANES_ZCASH_RPC_URL not set")?;
    let key_hex = std::env::var("ZALKANES_SIGNING_KEY").context("ZALKANES_SIGNING_KEY not set")?;
    let key_bytes = hex::decode(&key_hex)?;
    let key_arr: [u8; 32] = key_bytes
        .try_into()
        .map_err(|_| anyhow::anyhow!("32-byte key"))?;
    let key = SigningKey::from_secret_bytes(key_arr)?;

    let network = zalkanes_core::types::Network::Testnet;
    let rpc = Rpc::new(&rpc_url);
    let branch_id = zalkanes_core::branch_id_for_height(network, rpc.tip_height()?);

    let wasm = std::fs::read(&wasm_path)?;
    let code_hash = zalkanes_core::types::CodeHash::of(&wasm);
    let chunks = split_chunks(&wasm)?;
    let chunk_count = chunks.len() as u8;
    println!(
        "wasm: {wasm_path} size={} code_hash={} chunks={}",
        wasm.len(),
        code_hash.as_hex(),
        chunk_count
    );

    let addr = key
        .p2pkh_address()
        .to_zcash_address(NetworkType::Test)
        .to_string();
    println!("funding addr: {addr}");

    // Wait for PREPARE to be mined.
    println!("waiting for PREPARE {prepare_txid} to confirm...");
    let prepare_tx = rpc.wait_confirmed(&prepare_txid)?;
    println!("PREPARE confirmed at height {}", prepare_tx["height"]);

    // Build the DEPLOY OP_RETURN.
    let deploy_msg = zalkanes_protocol::DeployMessage {
        code_hash,
        code_length: wasm.len() as u32,
        chunk_count,
        output_index: 0,
    };
    let op_return = zalkanes_protocol::encode_deploy(&deploy_msg);

    // Carrier outpoints = PREPARE outputs 0..chunk_count-1.
    let carrier_value = 1_000_000u64;
    let carrier_values = vec![carrier_value; chunk_count as usize];
    let mut carrier_outpoints = Vec::with_capacity(chunk_count as usize);
    for i in 0..chunk_count {
        carrier_outpoints.push(OutPoint::new(
            rpc_txid_to_internal(&prepare_txid)?,
            i as u32,
        ));
    }

    let deploy = build_deploy(
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

    let deploy_tx = rpc.wait_confirmed(&deploy_txid)?;
    let deploy_height = deploy_tx["height"].as_u64().unwrap_or(0);
    println!("deploy_block_height: {deploy_height}");

    let txid_internal = rpc_txid_to_internal(&deploy_txid)?;
    let contract_id = zalkanes_core::types::ContractId::derive(
        network,
        &zalkanes_core::types::TxId(txid_internal),
        0,
        &code_hash,
    );
    println!("contract_id: {}", contract_id.as_hex());

    Ok(())
}
