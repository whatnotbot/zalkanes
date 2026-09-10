//! Carrier relay acceptance test against the LIVE Zebra regtest.
//!
//! Proves the ADR-0003 carrier end-to-end:
//!
//!   generate key → mine funding (P2PKH) → PREPARE (P2SH carrier output)
//!   → DEPLOY (spend carrier with chunk data in scriptSig) → mine
//!   → reconstruct chunk bytes from the block → verify byte-for-byte.
//!
//! Usage:
//! ```text
//! ZEBRA_RPC_URL=http://127.0.0.1:18232 \
//!   cargo run --example carrier_relay -- <chunk_payload_bytes>
//! ```

use anyhow::{bail, Context, Result};
use serde_json::Value;
use zalkanes_tx::{
    build_transparent_tx, carrier_script_sig, p2pkh_script_pubkey, p2sh_script_pubkey,
    redeem_script, zip317_fee, SigningKey, SpendInput, SpendKind, SpendOutput, CHUNK_PAYLOAD_SIZE,
    MAX_SIGNATURE_LEN,
};
use zcash_primitives::transaction::Transaction;
use zcash_protocol::{consensus::BlockHeight, value::Zatoshis};
use zcash_transparent::bundle::{OutPoint, TxOut};

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

    fn generate_to_address(&self, blocks: u32, addr: &str) -> Result<Vec<String>> {
        let v = self.call("generatetoaddress", serde_json::json!([blocks, addr]))?;
        Ok(v.as_array()
            .unwrap_or(&vec![])
            .iter()
            .filter_map(|s| s.as_str().map(|s| s.to_string()))
            .collect())
    }

    fn send_raw_transaction(&self, hex: &str) -> Result<String> {
        let v = self.call("sendrawtransaction", serde_json::json!([hex]))?;
        Ok(v.as_str().unwrap_or("").to_string())
    }

    fn get_block(&self, hash: &str, verbosity: u8) -> Result<Value> {
        self.call("getblock", serde_json::json!([hash, verbosity]))
    }

    fn get_raw_transaction(&self, txid: &str, verbosity: u8) -> Result<Value> {
        self.call("getrawtransaction", serde_json::json!([txid, verbosity]))
    }
}

/// Parse a txid from RPC display order into internal byte order.
fn rpc_txid_to_internal(display: &str) -> Result<[u8; 32]> {
    let bytes = hex::decode(display).context("decode txid hex")?;
    if bytes.len() != 32 {
        bail!("txid wrong length");
    }
    // RPC display order is byte-reversed; internal order is the reverse.
    let mut internal = [0u8; 32];
    internal.copy_from_slice(&bytes);
    internal.reverse();
    Ok(internal)
}

/// Find the first mature coinbase UTXO sent to our P2PKH address.
fn find_funding_utxo(rpc: &Rpc, addr: &str, blocks: &[String]) -> Result<(OutPoint, u64)> {
    for block_hash in blocks {
        let block = rpc.get_block(block_hash, 1)?;
        let confirmations = block["confirmations"].as_u64().unwrap_or(0);
        if confirmations < 100 {
            continue;
        }
        let txid = block["tx"][0].as_str().context("no coinbase txid")?;
        let tx = rpc.get_raw_transaction(txid, 1)?;
        for vout in tx["vout"].as_array().unwrap_or(&vec![]) {
            let _script = vout["scriptPubKey"]["hex"].as_str().unwrap_or("");
            // Check it pays to our P2PKH address.
            let empty: Vec<Value> = Vec::new();
            let addrs = vout["scriptPubKey"]["addresses"]
                .as_array()
                .unwrap_or(&empty);
            if addrs.iter().any(|a| a.as_str() == Some(addr)) {
                let value_zat = vout["valueZat"].as_u64().context("valueZat")?;
                let n = vout["n"].as_u64().context("n")? as u32;
                let outpoint = OutPoint::new(rpc_txid_to_internal(txid)?, n);
                return Ok((outpoint, value_zat));
            }
        }
    }
    bail!("no mature funding UTXO found for {addr}")
}

fn main() -> Result<()> {
    let rpc_url = std::env::var("ZEBRA_RPC_URL").context("ZEBRA_RPC_URL not set")?;
    let payload_len: usize = std::env::args()
        .nth(1)
        .unwrap_or_else(|| CHUNK_PAYLOAD_SIZE.to_string())
        .parse()
        .context("chunk payload bytes arg")?;
    if payload_len > CHUNK_PAYLOAD_SIZE {
        bail!("payload {payload_len} exceeds frozen CHUNK_PAYLOAD_SIZE {CHUNK_PAYLOAD_SIZE}");
    }

    let rpc = Rpc::new(&rpc_url);
    let key = SigningKey::dev_key();
    let pubkey = key.compressed_pubkey();
    let addr = key
        .p2pkh_address()
        .to_zcash_address(zcash_protocol::consensus::NetworkType::Regtest)
        .to_string();
    println!("funding address: {addr}");

    // 1. Mine 110 blocks to our address (coinbase maturity = 100).
    println!("mining 110 blocks to funding address...");
    let blocks = rpc.generate_to_address(110, &addr)?;
    println!("mined {} blocks", blocks.len());

    // 2. Find a mature funding UTXO.
    let (funding_outpoint, funding_value) = find_funding_utxo(&rpc, &addr, &blocks)?;
    println!(
        "funding UTXO: value={funding_value} zat, outpoint=({}, {})",
        hex::encode(funding_outpoint.hash()),
        funding_outpoint.n()
    );

    // 3. Build the redeem script + carrier P2SH output.
    let redeem = redeem_script(&pubkey);
    let redeem_p2sh = p2sh_script_pubkey(&redeem);
    println!("redeem script: {}", hex::encode(&redeem));
    println!("redeem sigops: 1 (OP_CHECKSIG)");

    // 4. PREPARE: fund one carrier P2SH output.
    let carrier_value = 1_000_000u64; // 0.01 ZEC
    let prepare_fee = zip317_fee(1, 2);
    let change_value = funding_value
        .checked_sub(carrier_value)
        .and_then(|v| v.checked_sub(prepare_fee))
        .context("funding UTXO too small")?;

    let prepare = build_transparent_tx(
        &[SpendInput {
            outpoint: funding_outpoint.clone(),
            value: funding_value,
            script_pubkey: p2pkh_script_pubkey(&pubkey),
            script_code: p2pkh_script_pubkey(&pubkey),
            kind: SpendKind::P2pkh { key: key.clone() },
        }],
        &[
            SpendOutput {
                value: carrier_value,
                script_pubkey: redeem_p2sh.clone(),
            },
            SpendOutput {
                value: change_value,
                script_pubkey: p2pkh_script_pubkey(&pubkey),
            },
        ],
        0,
    )?;
    let prepare_txid_display = prepare.txid_hex();
    println!("PREPARE txid: {prepare_txid_display}");
    println!("PREPARE fee: {prepare_fee} zat");

    let prepare_accepted = rpc.send_raw_transaction(&hex::encode(&prepare.bytes))?;
    println!("PREPARE accepted: {prepare_accepted}");
    assert_eq!(prepare_accepted, prepare_txid_display);

    // Mine PREPARE into a block.
    let mine1 = rpc.generate_to_address(1, &addr)?;
    println!("PREPARE mined in block: {}", mine1[0]);

    // 5. Build the chunk data.
    let mut chunk_data = vec![0u8; payload_len];
    for (i, b) in chunk_data.iter_mut().enumerate() {
        *b = (i % 251) as u8;
    }

    // 6. DEPLOY: spend the carrier P2SH output with chunk data in the scriptSig.
    let deploy_fee = zip317_fee(1, 1);
    // OP_RETURN output (value 0) carries a dummy payload for this relay test.
    let op_return = vec![0x6a, 0x04, 0x5a, 0x41, 0x4c, 0x4b]; // OP_RETURN "ZALK"
    let deploy = build_transparent_tx(
        &[SpendInput {
            outpoint: OutPoint::new(rpc_txid_to_internal(&prepare_txid_display)?, 0),
            value: carrier_value,
            script_pubkey: redeem_p2sh.clone(),
            script_code: redeem.clone(),
            kind: SpendKind::Carrier {
                key: key.clone(),
                chunk_index: 0,
                chunk_data: chunk_data.clone(),
                redeem_script: redeem.clone(),
            },
        }],
        &[
            // OP_RETURN output
            SpendOutput {
                value: 0,
                script_pubkey: op_return.clone(),
            },
        ],
        0,
    )?;
    let deploy_txid_display = deploy.txid_hex();
    println!("DEPLOY txid: {deploy_txid_display}");
    println!("DEPLOY fee: {deploy_fee} zat");
    println!("scriptSig serialized bytes: {}", {
        let ss = carrier_script_sig(0, &chunk_data, &[0x30; MAX_SIGNATURE_LEN], &redeem);
        ss.len()
    });
    println!("chunk payload bytes: {payload_len}");

    let deploy_accepted = rpc.send_raw_transaction(&hex::encode(&deploy.bytes))?;
    println!("DEPLOY accepted: {deploy_accepted}");
    assert_eq!(deploy_accepted, deploy_txid_display);

    // Mine DEPLOY.
    let mine2 = rpc.generate_to_address(1, &addr)?;
    println!("DEPLOY mined in block: {}", mine2[0]);

    // 7. Reconstruct: read the DEPLOY tx, extract the carrier chunk, verify.
    let tx_json = rpc.get_raw_transaction(&deploy_txid_display, 1)?;
    let script_sig_hex = tx_json["vin"][0]["scriptSig"]["hex"]
        .as_str()
        .context("scriptSig hex")?;
    let script_sig = hex::decode(script_sig_hex)?;

    // Extract chunk_data: pushes[0]=chunk_index, pushes[1..n-2]=chunk_data parts,
    // pushes[n-2]=signature, pushes[n-1]=redeem_script.
    let pushes = parse_pushes(&script_sig);
    println!("scriptSig push count: {}", pushes.len());
    assert!(
        pushes.len() >= 4,
        "expected index, chunk parts, sig, redeem"
    );
    assert_eq!(pushes[0], vec![0u8], "chunk_index should be 0");
    let mut reconstructed = Vec::new();
    for part in &pushes[1..pushes.len() - 2] {
        reconstructed.extend_from_slice(part);
    }
    assert_eq!(
        reconstructed, chunk_data,
        "chunk_data must round-trip byte-for-byte"
    );
    assert_eq!(
        pushes[pushes.len() - 1],
        redeem,
        "redeem script must round-trip"
    );

    println!(
        "chunk_data reconstruction: OK ({} bytes identical)",
        reconstructed.len()
    );

    // Print the authoritative block record.
    let block = rpc.get_block(&mine2[0], 1)?;
    println!(
        "DEPLOY block height: {}, hash: {}",
        block["height"].as_u64().unwrap_or(0),
        mine2[0]
    );

    println!("\nCARRIER RELAY ACCEPTANCE PASSED");
    Ok(())
}

/// Parse a push-only script into its data pushes.
fn parse_pushes(script: &[u8]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < script.len() {
        let op = script[i];
        let (len, data_start) = match op {
            0x00 => {
                out.push(Vec::new());
                i += 1;
                continue;
            }
            0x01..=0x4b => (op as usize, i + 1),
            0x4c => {
                let n = script[i + 1] as usize;
                (n, i + 2)
            }
            0x4d => {
                let n = u16::from_le_bytes([script[i + 1], script[i + 2]]) as usize;
                (n, i + 3)
            }
            _ => break,
        };
        out.push(script[data_start..data_start + len].to_vec());
        i = data_start + len;
    }
    out
}

// Silence unused warnings for types pulled in for clarity.
#[allow(dead_code)]
fn _unused(_t: Transaction, _b: BlockHeight, _z: Zatoshis, _o: TxOut) {}
