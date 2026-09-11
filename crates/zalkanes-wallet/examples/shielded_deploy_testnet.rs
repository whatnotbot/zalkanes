//! Item 14 live acceptance: shielded (Ironwood) funded PREPARE creating
//! public P2SH carrier UTXOs, then a transparent DEPLOY spending them (WASM
//! chunks in the carrier scriptSigs), through the full verification
//! boundaries, on public testnet.
//!
//! Broadcast only happens when ZALKANES_BROADCAST=1; otherwise stops after
//! verifying the PREPARE and prints its hex.

#![forbid(unsafe_code)]

use std::rc::Rc;
use zalkanes_core::consensus_params::ConsensusParams;

use anyhow::{anyhow, bail, Result};
use secrecy::SecretVec;
use zalkanes_wallet::{
    funding::TipSource, FundContext, FundingSource, FundingUtxo, ShieldedFunding, ShieldedWallet,
    SqliteShieldedWallet, TransparentFunding, TxRequest, ZebraCanonicalChainSource,
};

struct SharedWallet(Rc<SqliteShieldedWallet>);

impl ShieldedWallet for SharedWallet {
    fn sync_status(
        &self,
        tip: zalkanes_wallet::CanonicalTip,
    ) -> Result<zalkanes_wallet::SyncStatus> {
        ShieldedWallet::sync_status(&*self.0, tip)
    }
    fn select_spends(
        &self,
        required_zat: u64,
        plan_id: &str,
    ) -> Result<zalkanes_wallet::ShieldedSelection> {
        ShieldedWallet::select_spends(&*self.0, required_zat, plan_id)
    }
    fn release(&self, plan_id: &str) -> Result<()> {
        ShieldedWallet::release(&*self.0, plan_id)
    }
}

fn env_or(name: &str, default: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| default.to_string())
}

fn rpc_call(url: &str, method: &str, params: serde_json::Value) -> Result<serde_json::Value> {
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()?;
    let resp: serde_json::Value = client
        .post(url)
        .json(&serde_json::json!({"jsonrpc":"2.0","id":1,"method":method,"params":params}))
        .send()?
        .json()?;
    if let Some(err) = resp.get("error").filter(|e| !e.is_null()) {
        bail!("{method} error: {err}");
    }
    Ok(resp
        .get("result")
        .cloned()
        .unwrap_or(serde_json::Value::Null))
}

fn wait_mined(zebra_url: &str, txid_hex: &str) -> Result<u64> {
    for _ in 0..90 {
        std::thread::sleep(std::time::Duration::from_secs(10));
        if let Ok(tx) = rpc_call(
            zebra_url,
            "getrawtransaction",
            serde_json::json!([txid_hex, 1]),
        ) {
            if let Some(h) = tx.get("height").and_then(|h| h.as_u64()) {
                if tx
                    .get("confirmations")
                    .and_then(|c| c.as_u64())
                    .unwrap_or(0)
                    >= 1
                {
                    return Ok(h);
                }
            }
        }
    }
    bail!("not mined within 15 minutes")
}

fn display_txid(internal: [u8; 32]) -> String {
    let mut d = internal;
    d.reverse();
    hex::encode(d)
}

/// Load (or create, mode 600) the transparent carrier key.
fn carrier_key(wallet_dir: &std::path::Path) -> Result<zalkanes_tx::SigningKey> {
    use std::io::Write;
    let path = wallet_dir.join("carrier_key");
    if path.exists() {
        let hex_str = std::fs::read_to_string(&path)?;
        let mut bytes = [0u8; 32];
        hex::decode_to_slice(hex_str.trim(), &mut bytes)?;
        return zalkanes_tx::SigningKey::from_secret_bytes(bytes);
    }
    let mut bytes = [0u8; 32];
    use rand_core::RngCore;
    rand_core::OsRng.fill_bytes(&mut bytes);
    let key = zalkanes_tx::SigningKey::from_secret_bytes(bytes)?;
    let mut f = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&path)?;
    f.write_all(hex::encode(bytes).as_bytes())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(key)
}

fn main() -> Result<()> {
    let zebra_url = env_or(
        "ZALKANES_TESTNET_RPC_URL",
        "http://altaria.proxy.rlwy.net:14833",
    );
    let zalkanes_url = env_or(
        "ZALKANES_RPC",
        "https://zalkanes-testnet-production.up.railway.app",
    );
    let wallet_dir = std::path::PathBuf::from(env_or(
        "ZALKANES_WALLET_DIR",
        &format!(
            "{}/.zalkanes/wallet/testnet",
            std::env::var("HOME").unwrap_or_else(|_| ".".into())
        ),
    ));
    let wasm_path = env_or(
        "ZALKANES_WASM",
        "crates/zalkanes-testkit/fixtures/counter.wasm",
    );
    let broadcast = env_or("ZALKANES_BROADCAST", "0") == "1";
    const MAX_FEE_ZAT: u64 = 200_000;

    // ── WASM + DEPLOY message ───────────────────────────────────────────────
    let wasm = std::fs::read(&wasm_path).map_err(|e| anyhow!("read {wasm_path}: {e}"))?;
    let chunks = zalkanes_tx::split_chunks(&wasm)?;
    let code_hash = zalkanes_core::types::CodeHash::of(&wasm);
    let deploy_msg = zalkanes_protocol::DeployMessage {
        code_hash,
        code_length: wasm.len() as u32,
        chunk_count: chunks.len() as u8,
        output_index: 0,
    };
    let deploy_op_return = zalkanes_protocol::encode_deploy(&deploy_msg);
    println!(
        "wasm: {} bytes, {} chunks, code hash {}",
        wasm.len(),
        chunks.len(),
        hex::encode(code_hash.0)
    );

    // ── Keys + wallet + chain ───────────────────────────────────────────────
    let key = carrier_key(&wallet_dir)?;
    let seed_hex = std::fs::read_to_string(wallet_dir.join("seed"))?;
    let seed = SecretVec::new(hex::decode(seed_hex.trim())?);
    let chain_source = ZebraCanonicalChainSource::new(zebra_url.clone(), ConsensusParams::Test)?;
    let wallet = Rc::new(SqliteShieldedWallet::reopen(
        &wallet_dir.join("wallet.sqlite"),
        ConsensusParams::Test,
        seed,
    )?);
    let funding = ShieldedFunding::new(
        Box::new(SharedWallet(Rc::clone(&wallet))),
        Some(key.clone()),
    );

    // ── Size the carriers from the exact DEPLOY fee ─────────────────────────
    // Compute the DEPLOY fee with dummy outpoints (fee depends on sizes only),
    // then fund each carrier so the total covers fee + a small change.
    let n = chunks.len() as u64;
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
    let carrier_each = (deploy_fee + 50_000).div_ceil(n);
    let carrier_values = vec![carrier_each; chunks.len()];
    println!(
        "deploy fee: {deploy_fee} zat; carriers: {} x {carrier_each} zat",
        chunks.len()
    );
    if deploy_fee > MAX_FEE_ZAT {
        bail!("deploy fee {deploy_fee} exceeds safety cap {MAX_FEE_ZAT}");
    }

    // ── Stage 1: shielded PREPARE ───────────────────────────────────────────
    println!("pre-warming Orchard proving key…");
    let _ = zcash_primitives::transaction::builder::cached_orchard_proving_key(
        orchard::circuit::OrchardCircuitVersion::PostNu6_3,
    );

    let mut prepared = None;
    for attempt in 1..=8 {
        wallet.scan_to_tip(&chain_source)?;
        let tip = chain_source.canonical_tip()?;
        println!("PREPARE attempt {attempt} at tip {}", tip.height);
        let ctx = FundContext::new(zalkanes_core::types::Network::Testnet, tip);
        let mut plan = funding.plan(
            &TxRequest::Prepare {
                carrier_values: carrier_values.clone(),
            },
            &ctx,
        )?;
        println!("{}", plan.describe());
        if plan.fee() > MAX_FEE_ZAT {
            bail!("prepare fee {} exceeds safety cap", plan.fee());
        }
        let plan_id = plan.plan_id().to_string();
        let step = (|| -> Result<zalkanes_wallet::plan::VerifiedTransaction> {
            plan.prove(&chain_source)?;
            plan.sign(&chain_source)?;
            plan.extract_verified(&chain_source)
        })();
        match step {
            Ok(v) => {
                prepared = Some((v, plan.fee()));
                break;
            }
            Err(e) if e.to_string().contains("StalePlan") => {
                println!("stale plan; retrying");
                let _ = ShieldedWallet::release(&*wallet, &plan_id);
                continue;
            }
            Err(e) => {
                let _ = ShieldedWallet::release(&*wallet, &plan_id);
                return Err(e);
            }
        }
    }
    let (prepare_tx, prepare_fee) = prepared.ok_or_else(|| anyhow!("no fresh PREPARE plan"))?;
    let prepare_txid_hex = display_txid(prepare_tx.txid());
    println!("PREPARE verified: txid {prepare_txid_hex}, fee {prepare_fee} zat");

    if !broadcast {
        println!("ZALKANES_BROADCAST != 1 — stopping before broadcast.");
        println!("prepare tx hex: {}", hex::encode(prepare_tx.bytes()));
        return Ok(());
    }

    rpc_call(
        &zebra_url,
        "sendrawtransaction",
        serde_json::json!([hex::encode(prepare_tx.bytes())]),
    )?;
    let prepare_height = wait_mined(&zebra_url, &prepare_txid_hex)?;
    println!("PREPARE mined at {prepare_height}");

    // ── Stage 2: transparent DEPLOY spending the carriers ───────────────────
    let carrier_outpoints: Vec<zalkanes_tx::OutPoint> = (0..chunks.len())
        .map(|i| zalkanes_tx::OutPoint::new(prepare_tx.txid(), i as u32))
        .collect();
    let carrier_utxos: Vec<FundingUtxo> = carrier_outpoints
        .iter()
        .zip(&carrier_values)
        .map(|(op, v)| FundingUtxo {
            outpoint: op.clone(),
            value: *v,
        })
        .collect();

    let tip = chain_source.canonical_tip()?;
    let ctx = FundContext::new(zalkanes_core::types::Network::Testnet, tip);
    let tf = TransparentFunding::new(key.clone(), carrier_utxos);
    let mut deploy_plan = tf.plan(
        &TxRequest::Deploy {
            chunks: chunks.clone(),
            carrier_outpoints,
            carrier_values: carrier_values.clone(),
            op_return: deploy_op_return.clone(),
        },
        &ctx,
    )?;
    println!("{}", deploy_plan.describe());
    deploy_plan.prove(&chain_source)?;
    deploy_plan.sign(&chain_source)?;
    let deploy_tx = deploy_plan.extract_verified(&chain_source)?;
    let deploy_txid_hex = display_txid(deploy_tx.txid());
    println!("DEPLOY verified: txid {deploy_txid_hex}");

    rpc_call(
        &zebra_url,
        "sendrawtransaction",
        serde_json::json!([hex::encode(deploy_tx.bytes())]),
    )?;
    let deploy_height = wait_mined(&zebra_url, &deploy_txid_hex)?;
    println!("DEPLOY mined at {deploy_height}");

    // ── Contract identity + indexer verification ────────────────────────────
    let contract_id = zalkanes_core::types::ContractId::derive(
        zalkanes_core::types::Network::Testnet,
        &zalkanes_core::types::TxId(deploy_tx.txid()),
        0,
        &code_hash,
    );
    let contract_hex = hex::encode(contract_id.0);
    println!("derived ContractId: {contract_hex}");

    let mut info = serde_json::Value::Null;
    for _ in 0..60 {
        info = rpc_call(&zalkanes_url, "zalkanes_getInfo", serde_json::json!([]))?;
        if info
            .get("indexed_height")
            .and_then(|h| h.as_u64())
            .unwrap_or(0)
            >= deploy_height
        {
            break;
        }
        std::thread::sleep(std::time::Duration::from_secs(5));
    }
    println!(
        "indexer: height={} root={}",
        info["indexed_height"], info["state_root"]
    );
    let contract = rpc_call(
        &zalkanes_url,
        "zalkanes_getContract",
        serde_json::json!([contract_hex]),
    )?;
    println!("contract record: {contract}");
    let view = rpc_call(
        &zalkanes_url,
        "zalkanes_view",
        serde_json::json!([contract_hex, 2, ""]),
    )?;
    println!("fresh counter get(): {view}");
    let got = view
        .get("output_hex")
        .and_then(|v| v.as_str())
        .map(|s| u64::from_str_radix(s, 16))
        .transpose()?
        .ok_or_else(|| anyhow!("view returned no output"))?;
    if got != 0 {
        bail!("fresh counter must read 0, got {got}");
    }
    println!("fresh counter reads 0 ✓");
    println!("next: run shielded_call_testnet with ZALKANES_CONTRACT_ID={contract_hex}");
    Ok(())
}
