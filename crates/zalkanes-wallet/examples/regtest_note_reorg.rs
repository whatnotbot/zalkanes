//! Live note-level wallet reorg acceptance against two real zebrad regtest
//! nodes (§ mainnet gate: "live note-level competing-branch reorg").
//!
//! This is NOT a simulation. Every block is produced by a real pinned zebrad,
//! every shielded note is a real Ironwood note created by a real shielded
//! coinbase, the reorg is performed by Zebra's own best-work chain selection,
//! and the wallet is the real `SqliteShieldedWallet` driving the real
//! `rewind_to_chain_state` / rescan path. Nothing touches the wallet SQLite or
//! the Zalkanes RocksDB directly.
//!
//! Topology (Design A — two nodes, blocks moved by RPC, no P2P):
//!
//! ```text
//!   common ancestor H
//!         |
//!         +---- branch A ---- A1 ---- A2          (node A, contains our tx)
//!         |
//!         +---- branch B ---- B1 ---- B2 ---- B3  (node B, does not)
//! ```
//!
//! Branch B is longer, so when its blocks are submitted to node A, Zebra
//! reorganises onto it under normal consensus rules.
//!
//! CASE A — a received note disappears when its branch is removed.
//! CASE B — a spend rolls back and the note becomes spendable again.
//!
//! Usage: `regtest_note_reorg <case-a|case-b>`

#![forbid(unsafe_code)]

use std::rc::Rc;

use anyhow::{anyhow, bail, Result};
use secrecy::SecretVec;
use zalkanes_core::consensus_params::ConsensusParams;
use zalkanes_wallet::{
    funding::TipSource, FundContext, FundingSource, ShieldedFunding, ShieldedWallet,
    SqliteShieldedWallet, TxRequest, ZebraCanonicalChainSource,
};

/// Trait-object adapter so the driver keeps a handle to the same wallet the
/// funding source owns.
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

// ── Minimal JSON-RPC 1.0 client ─────────────────────────────────────────────

struct Node {
    name: &'static str,
    url: String,
    client: reqwest::blocking::Client,
}

impl Node {
    fn new(name: &'static str, url: String) -> Result<Self> {
        Ok(Self {
            name,
            url,
            client: reqwest::blocking::Client::builder()
                .timeout(std::time::Duration::from_secs(120))
                .build()?,
        })
    }

    fn call(&self, method: &str, params: serde_json::Value) -> Result<serde_json::Value> {
        let resp: serde_json::Value = self
            .client
            .post(&self.url)
            .json(&serde_json::json!({"jsonrpc":"1.0","id":"harness","method":method,"params":params}))
            .send()?
            .json()?;
        if let Some(err) = resp.get("error").filter(|e| !e.is_null()) {
            bail!("{}: {method} failed: {err}", self.name);
        }
        Ok(resp
            .get("result")
            .cloned()
            .unwrap_or(serde_json::Value::Null))
    }

    fn height(&self) -> Result<u64> {
        self.call("getblockcount", serde_json::json!([]))?
            .as_u64()
            .ok_or_else(|| anyhow!("{}: getblockcount returned no number", self.name))
    }

    fn block_hash(&self, height: u64) -> Result<String> {
        Ok(self
            .call("getblockhash", serde_json::json!([height]))?
            .as_str()
            .ok_or_else(|| anyhow!("{}: getblockhash returned no string", self.name))?
            .to_string())
    }

    /// Raw block hex. Only readable while the block is on THIS node's best
    /// chain — Zebra's `getblock` is best-chain-only at every verbosity.
    fn block_hex(&self, height: u64) -> Result<String> {
        Ok(self
            .call("getblock", serde_json::json!([height.to_string(), 0]))?
            .as_str()
            .ok_or_else(|| anyhow!("{}: getblock returned no hex", self.name))?
            .to_string())
    }

    fn generate(&self, n: u64) -> Result<()> {
        self.call("generate", serde_json::json!([n]))?;
        Ok(())
    }

    /// Mine one block whose coinbase pays `addr`. With a Unified address on
    /// NU6.3 this produces a real shielded (Ironwood) coinbase note.
    fn generate_to_address(&self, n: u64, addr: &str) -> Result<()> {
        self.call("generatetoaddress", serde_json::json!([n, addr]))?;
        Ok(())
    }

    fn submit_block(&self, hex: &str) -> Result<String> {
        let r = self.call("submitblock", serde_json::json!([hex]))?;
        // `null` means accepted, but NOT necessarily that it became the tip —
        // Zebra never returns "inconclusive" for side chains. The caller must
        // confirm the tip separately.
        Ok(match r {
            serde_json::Value::Null => "accepted".to_string(),
            other => other.to_string(),
        })
    }

    fn tx_confirmations(&self, txid: &str) -> Result<Option<i64>> {
        match self.call("getrawtransaction", serde_json::json!([txid, 1])) {
            Ok(v) => Ok(v.get("confirmations").and_then(|c| c.as_i64())),
            Err(_) => Ok(None),
        }
    }
}

/// Copy blocks `from..=to` out of `src` (while they are its best chain) and
/// submit them to `dst`, parent-first.
fn transplant(src: &Node, dst: &Node, from: u64, to: u64) -> Result<()> {
    for h in from..=to {
        let hex = src.block_hex(h)?;
        let res = dst.submit_block(&hex)?;
        if res != "accepted" && res != "\"duplicate\"" {
            bail!("{} rejected block {h} from {}: {res}", dst.name, src.name);
        }
    }
    Ok(())
}

// ── Evidence helpers ────────────────────────────────────────────────────────

struct Snapshot {
    scan_height: u32,
    balance: u64,
    notes: String,
    tip_height: u32,
    tip_hash: String,
}

fn snapshot(wallet: &SqliteShieldedWallet, cs: &ZebraCanonicalChainSource) -> Result<Snapshot> {
    let tip = cs.canonical_tip()?;
    Ok(Snapshot {
        scan_height: wallet.next_scan_height()?.saturating_sub(1),
        balance: wallet.balance()?,
        notes: wallet.shielded_note_summary()?,
        tip_height: tip.height,
        tip_hash: hex::encode(tip.hash),
    })
}

fn print_snapshot(label: &str, s: &Snapshot) {
    println!("  {label}:");
    println!("    wallet scanned to : {}", s.scan_height);
    println!("    balance           : {} zat", s.balance);
    println!("    notes             : {}", s.notes);
    println!("    canonical tip     : {}:{}", s.tip_height, s.tip_hash);
}

fn seed_from_env() -> Result<SecretVec<u8>> {
    // Deterministic throwaway regtest seed. This is a TEST seed for an
    // ephemeral regtest chain and holds no real value.
    let hex_seed = std::env::var("ZALKANES_REGTEST_SEED").unwrap_or_else(|_| {
        "5a616c6b616e6573526567746573745365656430313233343536373839414243".to_string()
    });
    let bytes = hex::decode(hex_seed.trim())?;
    if bytes.len() != 32 {
        bail!("regtest seed must be 32 bytes, got {}", bytes.len());
    }
    Ok(SecretVec::new(bytes))
}

// ── Shared setup ────────────────────────────────────────────────────────────

struct Harness {
    a: Node,
    b: Node,
    cs: ZebraCanonicalChainSource,
    wallet: Rc<SqliteShieldedWallet>,
    wallet_path: std::path::PathBuf,
}

/// Mine a common history on A, replicate it to B, and open a wallet whose
/// birthday sits below everything that matters.
fn setup(common_blocks: u64) -> Result<Harness> {
    let a = Node::new(
        "node-A",
        std::env::var("ZALKANES_NODE_A").unwrap_or_else(|_| "http://127.0.0.1:18232".into()),
    )?;
    let b = Node::new(
        "node-B",
        std::env::var("ZALKANES_NODE_B").unwrap_or_else(|_| "http://127.0.0.1:18242".into()),
    )?;

    let g_a = a.block_hash(0)?;
    let g_b = b.block_hash(0)?;
    if g_a != g_b {
        bail!("nodes do not share a genesis: {g_a} vs {g_b}");
    }
    println!("shared regtest genesis: {g_a}");

    println!("mining {common_blocks} common blocks on node A");
    a.generate(common_blocks)?;

    let dir = std::path::PathBuf::from(
        std::env::var("ZALKANES_WALLET_DIR")
            .unwrap_or_else(|_| "/tmp/zalkanes-reorg-wallet".into()),
    );
    std::fs::create_dir_all(&dir)?;
    let wallet_path = dir.join("wallet.sqlite");

    let cs = ZebraCanonicalChainSource::new(a.url.clone(), ConsensusParams::Regtest)?;
    // Birthday 2, not 1: `restore` reads the treestate at `birthday - 1`, and
    // regtest genesis (height 0) predates every network upgrade, so it has no
    // Orchard/Ironwood commitment trees to parse. Height 1 already has them.
    let wallet = Rc::new(SqliteShieldedWallet::restore(
        &wallet_path,
        ConsensusParams::Regtest,
        seed_from_env()?,
        2,
        &cs,
    )?);
    println!("wallet birthday: {}", wallet.birthday_height()?);

    Ok(Harness {
        a,
        b,
        cs,
        wallet,
        wallet_path,
    })
}

/// Mine one shielded-coinbase block paying the wallet, plus `extra` further
/// blocks so the note has confirmations. Returns the note block height.
fn mint_note(h: &Harness, extra: u64) -> Result<u64> {
    let ua = h.wallet.unified_address()?;
    println!("wallet unified address: {ua}");
    h.a.generate_to_address(1, &ua)?;
    let note_height = h.a.height()?;
    println!("shielded coinbase mined at height {note_height}");
    if extra > 0 {
        h.a.generate(extra)?;
    }
    Ok(note_height)
}

/// Build branch B from `fork` and make node A adopt it. Returns (tip height,
/// tip hash) of the new canonical chain.
fn force_reorg(h: &Harness, fork: u64, branch_b_len: u64) -> Result<(u64, String)> {
    // B must hold the common history before it can extend it.
    println!("replicating common history 1..={fork} into node B");
    transplant(&h.a, &h.b, 1, fork)?;
    let b_h = h.b.height()?;
    if b_h != fork {
        bail!("node B reached height {b_h}, expected the common ancestor {fork}");
    }
    let anc_a = h.a.block_hash(fork)?;
    let anc_b = h.b.block_hash(fork)?;
    if anc_a != anc_b {
        bail!("common ancestor mismatch at {fork}: {anc_a} vs {anc_b}");
    }
    println!("common ancestor confirmed at {fork}: {anc_a}");

    println!("mining {branch_b_len} blocks on node B (branch B)");
    h.b.generate(branch_b_len)?;
    let b_tip_h = h.b.height()?;
    let b_tip = h.b.block_hash(b_tip_h)?;
    let a_tip_h = h.a.height()?;
    let a_tip = h.a.block_hash(a_tip_h)?;
    println!("branch A tip: {a_tip_h} {a_tip}");
    println!("branch B tip: {b_tip_h} {b_tip}");
    if h.a.block_hash(fork + 1)? == h.b.block_hash(fork + 1)? {
        bail!("branches are identical at {} — no fork exists", fork + 1);
    }
    if b_tip_h <= a_tip_h {
        bail!("branch B ({b_tip_h}) is not longer than branch A ({a_tip_h})");
    }

    println!("submitting branch B into node A");
    transplant(&h.b, &h.a, fork + 1, b_tip_h)?;
    std::thread::sleep(std::time::Duration::from_secs(3));

    let new_h = h.a.height()?;
    let new_tip = h.a.block_hash(new_h)?;
    if new_h != b_tip_h || new_tip != b_tip {
        bail!("node A did not reorg: tip is {new_h} {new_tip}, expected {b_tip_h} {b_tip}");
    }
    println!("REORG CONFIRMED on node A: now {new_h} {new_tip}");
    Ok((new_h, new_tip))
}

// ── CASE A ──────────────────────────────────────────────────────────────────

fn case_a() -> Result<()> {
    println!("=== CASE A — a received note disappears with its branch ===");
    let h = setup(12)?;
    let fork = h.a.height()?;
    println!("common ancestor height H = {fork}");

    let note_height = mint_note(&h, 1)?;

    h.wallet.scan_to_tip(&h.cs)?;
    let before = snapshot(&h.wallet, &h.cs)?;
    print_snapshot("BEFORE REORG", &before);
    if before.balance == 0 {
        bail!("no shielded note was received on branch A — balance is 0");
    }
    if before.scan_height as u64 != h.a.height()? {
        bail!("wallet did not reach the canonical tip before the reorg");
    }
    let branch_a_note_block = h.a.block_hash(note_height)?;
    println!("  branch-A note block : {note_height} {branch_a_note_block}");

    // Branch B is longer and contains no shielded coinbase to us.
    let (new_tip_h, new_tip) = force_reorg(&h, fork, 4)?;

    h.wallet.scan_to_tip(&h.cs)?;
    let after = snapshot(&h.wallet, &h.cs)?;
    print_snapshot("AFTER REORG", &after);

    if after.balance != 0 {
        bail!(
            "the branch-A note is still spendable after the reorg (balance {} zat)",
            after.balance
        );
    }
    if after.scan_height as u64 != new_tip_h
        || after.tip_hash != {
            let mut v = hex::decode(&new_tip)?;
            v.reverse();
            hex::encode(v)
        }
    {
        bail!("wallet scan identity does not match the new canonical chain");
    }

    // Restart: reopen from disk and re-verify.
    drop(h.wallet);
    let reopened =
        SqliteShieldedWallet::reopen(&h.wallet_path, ConsensusParams::Regtest, seed_from_env()?)?;
    reopened.scan_to_tip(&h.cs)?;
    let restarted = snapshot(&reopened, &h.cs)?;
    print_snapshot("AFTER RESTART", &restarted);
    if restarted.balance != 0 {
        bail!("the removed note reappeared after a restart");
    }

    println!();
    println!("CASE A PASS");
    println!("  common ancestor      : {fork}");
    println!("  branch-A note block  : {note_height} {branch_a_note_block}");
    println!("  note visible before  : yes ({} zat)", before.balance);
    println!("  note absent after    : yes (0 zat)");
    println!("  canonical tip after  : {new_tip_h} {new_tip}");
    println!("  restart              : still absent");
    Ok(())
}

// ── CASE B ──────────────────────────────────────────────────────────────────

fn case_b() -> Result<()> {
    println!("=== CASE B — a spend rolls back and the note returns ===");
    let h = setup(12)?;

    // The note is received on history COMMON to both branches, so only the
    // spend is rolled back.
    let note_height = mint_note(&h, 2)?;
    h.wallet.scan_to_tip(&h.cs)?;
    let funded = snapshot(&h.wallet, &h.cs)?;
    print_snapshot("FUNDED (pre-fork, common history)", &funded);
    if funded.balance == 0 {
        bail!("no shielded note was received — balance is 0");
    }

    let fork = h.a.height()?;
    println!("common ancestor height H = {fork} (note at {note_height} is below it)");

    // ── Spend it with a real shielded transaction on branch A ───────────────
    println!("pre-warming the Orchard proving key");
    let _ = zcash_primitives::transaction::builder::cached_orchard_proving_key(
        orchard::circuit::OrchardCircuitVersion::PostNu6_3,
    );
    let op_return = zalkanes_protocol::encode_call(&zalkanes_protocol::CallMessage {
        contract_id: zalkanes_core::types::ContractId([0x11; 32]),
        opcode: 1,
        input: vec![],
    });
    let funding = ShieldedFunding::new(Box::new(SharedWallet(Rc::clone(&h.wallet))), None);
    let tip = h.cs.canonical_tip()?;
    let ctx = FundContext::new(zalkanes_core::types::Network::Regtest, tip);
    let mut plan = funding.plan(&TxRequest::Call { op_return }, &ctx)?;
    println!("{}", plan.describe());
    plan.prove(&h.cs)?;
    plan.sign(&h.cs)?;
    let verified = plan.extract_verified(&h.cs)?;
    let mut txid_display = verified.txid();
    txid_display.reverse();
    let spend_txid = hex::encode(txid_display);
    println!("spend transaction built: {spend_txid}");

    h.a.call(
        "sendrawtransaction",
        serde_json::json!([hex::encode(verified.bytes())]),
    )?;
    println!("spend broadcast to node A");
    h.a.generate(1)?;
    let spend_height = h.a.height()?;
    let conf = h.a.tx_confirmations(&spend_txid)?;
    println!("spend mined at height {spend_height} (confirmations {conf:?})");
    if conf.unwrap_or(0) < 1 {
        bail!("the spend was not mined on branch A");
    }

    h.wallet.scan_to_tip(&h.cs)?;
    let spent = snapshot(&h.wallet, &h.cs)?;
    print_snapshot("AFTER SPEND (branch A)", &spent);
    if spent.balance >= funded.balance {
        bail!(
            "balance did not fall after the spend ({} -> {})",
            funded.balance,
            spent.balance
        );
    }

    // ── Reorg onto a branch B that never saw the spend ──────────────────────
    let (new_tip_h, new_tip) = force_reorg(&h, fork, 4)?;
    let conf_after = h.a.tx_confirmations(&spend_txid)?;
    println!("spend confirmations after reorg: {conf_after:?}");

    h.wallet.scan_to_tip(&h.cs)?;
    let rolled_back = snapshot(&h.wallet, &h.cs)?;
    print_snapshot("AFTER REORG", &rolled_back);

    if rolled_back.balance != funded.balance {
        bail!(
            "the note did not become spendable again: expected {} zat, got {} zat",
            funded.balance,
            rolled_back.balance
        );
    }

    drop(h.wallet);
    let reopened =
        SqliteShieldedWallet::reopen(&h.wallet_path, ConsensusParams::Regtest, seed_from_env()?)?;
    reopened.scan_to_tip(&h.cs)?;
    let restarted = snapshot(&reopened, &h.cs)?;
    print_snapshot("AFTER RESTART", &restarted);
    if restarted.balance != funded.balance {
        bail!("the restored balance did not survive a restart");
    }

    println!();
    println!("CASE B PASS");
    println!("  common ancestor        : {fork}");
    println!("  note block (common)    : {note_height}");
    println!("  spend txid             : {spend_txid}");
    println!(
        "  spent before reorg     : yes (balance {} zat)",
        spent.balance
    );
    println!("  spend rolled back      : yes (confirmations {conf_after:?})");
    println!(
        "  spendable after reorg  : yes ({} zat, restored)",
        rolled_back.balance
    );
    println!("  canonical tip after    : {new_tip_h} {new_tip}");
    println!("  restart                : balance preserved");
    Ok(())
}

fn main() -> Result<()> {
    let which = std::env::args().nth(1).unwrap_or_else(|| "case-a".into());
    match which.as_str() {
        "case-a" => case_a(),
        "case-b" => case_b(),
        other => bail!("unknown case {other}; expected case-a or case-b"),
    }
}
