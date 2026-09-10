//! Multi-carrier relay test: deploy a synthetic N-chunk payload via the real
//! PREPARE + DEPLOY pipeline against the live Zebra regtest, then verify the
//! reconstruction. Establishes the maximum practical contract size.

use anyhow::{bail, Context, Result};
use serde_json::Value;
use zalkanes_tx::{build_deploy, build_prepare, op_return_script, split_chunks, SigningKey};
use zcash_protocol::consensus::BranchId;
use zcash_transparent::bundle::OutPoint;

struct Rpc {
    url: String,
    client: reqwest::blocking::Client,
}

impl Rpc {
    fn new(url: &str) -> Self {
        Rpc {
            url: url.to_string(),
            client: reqwest::blocking::Client::new(),
        }
    }
    fn call(&self, method: &str, params: Value) -> Result<Value> {
        let body = serde_json::json!({"jsonrpc":"2.0","id":1,"method":method,"params":params});
        let resp = self
            .client
            .post(&self.url)
            .header("Content-Type", "application/json")
            .json(&body)
            .send()?;
        let v: Value = resp.json()?;
        if let Some(e) = v.get("error") {
            if !e.is_null() {
                bail!("rpc {method}: {e}");
            }
        }
        Ok(v["result"].clone())
    }
    fn generate(&self, n: u32, addr: &str) -> Result<Vec<String>> {
        let v = self.call("generatetoaddress", serde_json::json!([n, addr]))?;
        Ok(v.as_array()
            .unwrap_or(&vec![])
            .iter()
            .filter_map(|s| s.as_str().map(String::from))
            .collect())
    }
    fn send(&self, hex: &str) -> Result<String> {
        let v = self.call("sendrawtransaction", serde_json::json!([hex]))?;
        Ok(v.as_str().unwrap_or("").to_string())
    }
    fn get_block(&self, h: &str) -> Result<Value> {
        self.call("getblock", serde_json::json!([h, 1]))
    }
    fn get_tx(&self, t: &str) -> Result<Value> {
        self.call("getrawtransaction", serde_json::json!([t, 1]))
    }
}

fn rpc_txid_to_internal(display: &str) -> Result<[u8; 32]> {
    let mut b = hex::decode(display)?;
    if b.len() != 32 {
        bail!("txid len");
    }
    b.reverse();
    let mut o = [0u8; 32];
    o.copy_from_slice(&b);
    Ok(o)
}

fn find_utxos(rpc: &Rpc, addr: &str, blocks: &[String]) -> Result<Vec<(OutPoint, u64)>> {
    let mut found = Vec::new();
    for bh in blocks {
        let blk = rpc.get_block(bh)?;
        if blk["confirmations"].as_u64().unwrap_or(0) < 100 {
            continue;
        }
        if let Some(txid) = blk["tx"][0].as_str() {
            let tx = rpc.get_tx(txid)?;
            let empty: Vec<Value> = Vec::new();
            for vo in tx["vout"].as_array().unwrap_or(&empty) {
                let addrs = vo["scriptPubKey"]["addresses"].as_array().unwrap_or(&empty);
                if addrs.iter().any(|a| a.as_str() == Some(addr)) {
                    let v = vo["valueZat"].as_u64().unwrap_or(0);
                    let n = vo["n"].as_u64().unwrap_or(0) as u32;
                    found.push((OutPoint::new(rpc_txid_to_internal(txid)?, n), v));
                }
            }
        }
    }
    if found.is_empty() {
        bail!("no utxo");
    }
    Ok(found)
}

fn main() -> Result<()> {
    let rpc_url = std::env::var("ZEBRA_RPC_URL").context("ZEBRA_RPC_URL")?;
    let payload_len: usize = std::env::args().nth(1).unwrap_or("65536".into()).parse()?;
    let rpc = Rpc::new(&rpc_url);
    let key = SigningKey::dev_key();
    let addr = key
        .p2pkh_address()
        .to_zcash_address(zcash_protocol::consensus::NetworkType::Regtest)
        .to_string();

    let wasm = vec![0xABu8; payload_len];
    let chunks = split_chunks(&wasm)?;
    let n = chunks.len();
    println!("payload {payload_len} bytes -> {n} chunks");

    // Fund.
    let blocks = rpc.generate(110, &addr)?;
    let funding = find_utxos(&rpc, &addr, &blocks)?;
    let total: u64 = funding.iter().map(|(_, v)| v).sum();
    println!("funding UTXOs: {} ({} zat total)", funding.len(), total);

    // PREPARE with N carriers. Each carrier only needs enough to pay its share
    // of the DEPLOY fee + dust threshold. Use 100_000 zatoshi per carrier.
    let carrier_val = 100_000u64;
    let values = vec![carrier_val; n];
    let prepare = build_prepare(&key, &funding, &values, BranchId::Canopy)?;
    let prepare_txid = prepare.txid_hex();
    assert_eq!(rpc.send(&hex::encode(&prepare.bytes))?, prepare_txid);
    let pb = rpc.generate(1, &addr)?;
    let pb_json = rpc.get_block(&pb[0])?;
    println!(
        "PREPARE {} mined at height {}",
        prepare_txid,
        pb_json["height"].as_u64().unwrap_or(0)
    );

    // DEPLOY.
    let mut outpoints = Vec::with_capacity(n);
    for i in 0..n {
        outpoints.push(OutPoint::new(
            rpc_txid_to_internal(&prepare_txid)?,
            i as u32,
        ));
    }
    let op_return = op_return_script(b"ZALK");
    let deploy = build_deploy(
        &key,
        &outpoints,
        &values,
        &chunks,
        &op_return,
        BranchId::Canopy,
    )?;
    let deploy_txid = deploy.txid_hex();
    assert_eq!(rpc.send(&hex::encode(&deploy.bytes))?, deploy_txid);
    let db = rpc.generate(1, &addr)?;
    let db_json = rpc.get_block(&db[0])?;
    println!(
        "DEPLOY {} mined at height {}",
        deploy_txid,
        db_json["height"].as_u64().unwrap_or(0)
    );

    // Verify: reconstruct all chunks from the DEPLOY tx scriptSigs.
    let tx = rpc.get_tx(&deploy_txid)?;
    let mut recovered = Vec::new();
    let empty: Vec<Value> = Vec::new();
    for vin in tx["vin"].as_array().unwrap_or(&empty) {
        let ss = hex::decode(vin["scriptSig"]["hex"].as_str().unwrap_or(""))?;
        if let Some((_idx, data)) = zalkanes_carrier::chunk_from_script_sig(&ss) {
            recovered.extend_from_slice(&data);
        }
    }
    println!("recovered {} bytes", recovered.len());
    assert_eq!(recovered, wasm, "recovered payload mismatch");

    println!("MULTI-CARRIER RELAY ({payload_len} bytes) PASSED");
    Ok(())
}
