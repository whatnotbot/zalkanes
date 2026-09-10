//! Minimal Zcash JSON-RPC client used by the CLI for deploy/call/fund.

#![forbid(unsafe_code)]

use anyhow::{bail, Context, Result};
use serde_json::Value;

pub struct ZcashRpc {
    url: String,
    client: reqwest::blocking::Client,
}

impl ZcashRpc {
    pub fn new(url: &str) -> Result<Self> {
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .context("build HTTP client")?;
        Ok(Self {
            url: url.to_string(),
            client,
        })
    }

    pub fn call(&self, method: &str, params: Value) -> Result<Value> {
        let body = serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": method, "params": params
        });
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

    pub fn generate_to_address(&self, blocks: u32, addr: &str) -> Result<Vec<String>> {
        let v = self.call("generatetoaddress", serde_json::json!([blocks, addr]))?;
        Ok(v.as_array()
            .unwrap_or(&vec![])
            .iter()
            .filter_map(|s| s.as_str().map(|s| s.to_string()))
            .collect())
    }

    pub fn send_raw_transaction(&self, hex: &str) -> Result<String> {
        let v = self.call("sendrawtransaction", serde_json::json!([hex]))?;
        Ok(v.as_str().unwrap_or("").to_string())
    }

    pub fn get_block(&self, hash: &str, verbosity: u8) -> Result<Value> {
        self.call("getblock", serde_json::json!([hash, verbosity]))
    }

    pub fn get_raw_transaction(&self, txid: &str, verbosity: u8) -> Result<Value> {
        self.call("getrawtransaction", serde_json::json!([txid, verbosity]))
    }
}

/// Parse an RPC display-order txid into internal byte order.
pub fn rpc_txid_to_internal(display: &str) -> Result<[u8; 32]> {
    let bytes = hex::decode(display).context("decode txid hex")?;
    if bytes.len() != 32 {
        bail!("txid wrong length");
    }
    let mut internal = [0u8; 32];
    internal.copy_from_slice(&bytes);
    internal.reverse();
    Ok(internal)
}

/// Find a mature (≥100 confirmations) UTXO paying to `addr`, from a list of
/// freshly mined block hashes.
pub fn find_mature_utxo(
    rpc: &ZcashRpc,
    addr: &str,
    blocks: &[String],
) -> Result<(zcash_transparent::bundle::OutPoint, u64)> {
    for block_hash in blocks {
        let block = rpc.get_block(block_hash, 1)?;
        let confirmations = block["confirmations"].as_u64().unwrap_or(0);
        if confirmations < 100 {
            continue;
        }
        let Some(txid) = block["tx"][0].as_str() else {
            continue;
        };
        let tx = rpc.get_raw_transaction(txid, 1)?;
        let empty: Vec<Value> = Vec::new();
        for vout in tx["vout"].as_array().unwrap_or(&empty) {
            let addrs = vout["scriptPubKey"]["addresses"]
                .as_array()
                .unwrap_or(&empty);
            if addrs.iter().any(|a| a.as_str() == Some(addr)) {
                let value = vout["valueZat"].as_u64().context("valueZat")?;
                let n = vout["n"].as_u64().context("n")? as u32;
                let outpoint =
                    zcash_transparent::bundle::OutPoint::new(rpc_txid_to_internal(txid)?, n);
                return Ok((outpoint, value));
            }
        }
    }
    bail!("no mature UTXO found for {addr}")
}
