//! Live testnet shielded CALL driver (Item 12 live acceptance).
//!
//! Flow: sync wallet → plan a shielded CALL (Ironwood/Orchard notes) → prove →
//! sign → extract through the VerifiedPczt + VerifiedTransaction boundaries →
//! broadcast via our Zebra → wait for mining → wait for Zalkanes indexing →
//! read back `get()` and the execution record.
//!
//! Broadcast only happens when ZALKANES_BROADCAST=1; otherwise the run stops
//! after verification and prints the transaction hex.

#![forbid(unsafe_code)]

use std::rc::Rc;
use zalkanes_core::consensus_params::ConsensusParams;

use anyhow::{anyhow, bail, Result};
use secrecy::SecretVec;
use zalkanes_wallet::{
    funding::TipSource, FundContext, FundingSource, ShieldedFunding, ShieldedWallet,
    SqliteShieldedWallet, TxRequest, ZebraCanonicalChainSource,
};

/// Trait-object adapter so the driver keeps a release/rescan handle to the
/// same wallet that the funding source owns.
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
    let contract_hex = env_or(
        "ZALKANES_CONTRACT_ID",
        "607a6246c512239a23f51cf8053444d4d76e7684c1a53dc26a626b474a8cf3c0",
    );
    let opcode: u16 = env_or("ZALKANES_OPCODE", "1").parse()?;
    let expect_get: Option<u64> = std::env::var("ZALKANES_EXPECT_GET")
        .ok()
        .map(|s| s.parse().expect("ZALKANES_EXPECT_GET must be a number"));
    let broadcast = env_or("ZALKANES_BROADCAST", "0") == "1";
    const MAX_FEE_ZAT: u64 = 100_000;

    // ── Build the CALL message ──────────────────────────────────────────────
    let mut contract_id = [0u8; 32];
    hex::decode_to_slice(&contract_hex, &mut contract_id)
        .map_err(|e| anyhow!("contract id hex: {e}"))?;
    let op_return = zalkanes_protocol::encode_call(&zalkanes_protocol::CallMessage {
        contract_id: zalkanes_core::types::ContractId(contract_id),
        opcode,
        input: vec![],
    });
    println!("ZALK payload: {}", hex::encode(&op_return));

    // ── Open wallet + chain source ──────────────────────────────────────────
    let seed_hex = std::fs::read_to_string(wallet_dir.join("seed"))?;
    let seed = SecretVec::new(hex::decode(seed_hex.trim())?);
    let chain_source = ZebraCanonicalChainSource::new(zebra_url.clone(), ConsensusParams::Test)?;
    let wallet = Rc::new(SqliteShieldedWallet::reopen(
        &wallet_dir.join("wallet.sqlite"),
        ConsensusParams::Test,
        seed,
    )?);
    let funding = ShieldedFunding::new(Box::new(SharedWallet(Rc::clone(&wallet))), None);

    // Pre-warm the Orchard proving key so the prove step fits comfortably
    // inside one testnet block interval (freshness window).
    println!("pre-warming Orchard proving key…");
    let _ = zcash_primitives::transaction::builder::cached_orchard_proving_key(
        orchard::circuit::OrchardCircuitVersion::PostNu6_3,
    );
    println!("proving key ready");

    // ── Plan/prove/sign/extract with stale-plan retry ───────────────────────
    let mut verified = None;
    for attempt in 1..=8 {
        wallet.scan_to_tip(&chain_source)?;
        let tip = chain_source.canonical_tip()?;
        println!(
            "attempt {attempt}: planning at tip {}:{}",
            tip.height,
            hex::encode(tip.hash)
        );
        let ctx = FundContext::new(zalkanes_core::types::Network::Testnet, tip);
        let mut plan = funding.plan(
            &TxRequest::Call {
                op_return: op_return.clone(),
            },
            &ctx,
        )?;
        println!("{}", plan.describe());
        if plan.fee() > MAX_FEE_ZAT {
            bail!("fee {} exceeds safety cap {MAX_FEE_ZAT}", plan.fee());
        }
        let plan_id = plan.plan_id().to_string();

        let step = (|| -> Result<zalkanes_wallet::plan::VerifiedTransaction> {
            plan.prove(&chain_source)?;
            println!("proved");
            plan.sign(&chain_source)?;
            println!("signed");
            plan.extract_verified(&chain_source)
        })();
        match step {
            Ok(v) => {
                verified = Some((v, plan.fee(), plan.change(), plan_id.clone()));
                break;
            }
            Err(e) if e.to_string().contains("StalePlan") => {
                println!("stale plan ({e}); releasing and retrying");
                let _ = ShieldedWallet::release(&*wallet, &plan_id);
                continue;
            }
            Err(e) => {
                let _ = ShieldedWallet::release(&*wallet, &plan_id);
                return Err(e);
            }
        }
    }
    let (verified, fee, change, plan_id) =
        verified.ok_or_else(|| anyhow!("no fresh plan within retry budget"))?;

    let mut txid_display = verified.txid();
    txid_display.reverse();
    let txid_hex = hex::encode(txid_display);
    println!("verified transaction:");
    println!("  txid:   {txid_hex}");
    println!("  size:   {} bytes", verified.bytes().len());
    println!("  fee:    {fee} zat");
    println!("  change: {change} zat");
    println!(
        "  verified at tip: {}:{}",
        verified.verified_tip().height,
        hex::encode(verified.verified_tip().hash)
    );

    if !broadcast {
        println!("ZALKANES_BROADCAST != 1 — stopping before broadcast.");
        println!("tx hex: {}", hex::encode(verified.bytes()));
        // Release the note reservation so a later (broadcasting) run can
        // select it; the dry-run transaction is discarded.
        let _ = ShieldedWallet::release(&*wallet, &plan_id);
        println!("note reservation released (dry run)");
        return Ok(());
    }

    // ── Broadcast through OUR Zebra ─────────────────────────────────────────
    // A broadcast REJECTION releases the note reservation (nothing is in
    // flight). After an ACCEPTED broadcast the reservation is intentionally
    // kept: the spend is in the mempool and the note must not be re-selected;
    // scanning marks it spent once mined.
    let sent = match rpc_call(
        &zebra_url,
        "sendrawtransaction",
        serde_json::json!([hex::encode(verified.bytes())]),
    ) {
        Ok(sent) => sent,
        Err(e) => {
            let _ = ShieldedWallet::release(&*wallet, &plan_id);
            println!("broadcast rejected; note reservation released");
            return Err(e);
        }
    };
    println!("broadcast accepted: {sent}");

    // ── Wait for mining ─────────────────────────────────────────────────────
    let mut mined_height: Option<u64> = None;
    for _ in 0..90 {
        std::thread::sleep(std::time::Duration::from_secs(10));
        let tx = rpc_call(
            &zebra_url,
            "getrawtransaction",
            serde_json::json!([txid_hex, 1]),
        );
        if let Ok(tx) = tx {
            if let Some(h) = tx.get("height").and_then(|h| h.as_u64()) {
                if tx
                    .get("confirmations")
                    .and_then(|c| c.as_u64())
                    .unwrap_or(0)
                    >= 1
                {
                    mined_height = Some(h);
                    break;
                }
            }
        }
    }
    let mined_height = mined_height.ok_or_else(|| anyhow!("not mined within 15 minutes"))?;
    let block_hash = rpc_call(
        &zebra_url,
        "getblockhash",
        serde_json::json!([mined_height]),
    )?;
    println!("mined: height={mined_height} block={block_hash}");

    // ── Wait for Zalkanes indexing, then read back state ────────────────────
    let mut info = serde_json::Value::Null;
    for _ in 0..60 {
        info = rpc_call(&zalkanes_url, "zalkanes_getInfo", serde_json::json!([]))?;
        if info
            .get("indexed_height")
            .and_then(|h| h.as_u64())
            .unwrap_or(0)
            >= mined_height
        {
            break;
        }
        std::thread::sleep(std::time::Duration::from_secs(5));
    }
    println!(
        "indexer: height={} root={}",
        info["indexed_height"], info["state_root"]
    );

    let view = rpc_call(
        &zalkanes_url,
        "zalkanes_view",
        serde_json::json!([contract_hex, 2, ""]),
    )?;
    println!("view get(): {view}");
    let got = view
        .get("output_hex")
        .and_then(|v| v.as_str())
        .map(|s| u64::from_str_radix(s, 16))
        .transpose()?
        .ok_or_else(|| anyhow!("view returned no output"))?;
    println!("counter value: {got}");
    if let Some(expected) = expect_get {
        if got != expected {
            bail!("counter mismatch: got {got}, expected {expected}");
        }
        println!("counter matches expected {expected}");
    }

    let exec = rpc_call(
        &zalkanes_url,
        "zalkanes_getExecution",
        serde_json::json!([txid_hex]),
    )?;
    println!("execution record: {exec}");
    Ok(())
}
