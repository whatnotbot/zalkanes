//! Production wallet + funded-transaction commands.
//!
//! # Custody model for an ephemeral CLI
//!
//! A one-shot CLI process cannot hold spend authorization in memory between
//! invocations, and persisting a decrypted seed would defeat the keystore.
//! So `unlock`/`lock` manage an **armed session that contains no secret**:
//!
//! - `wallet unlock` verifies the passphrase against the keystore and writes
//!   a session marker (no key material, owner-only, with an expiry).
//! - Spending commands require BOTH an armed session AND the passphrase
//!   (`$ZALKANES_PASSPHRASE`, or an interactive prompt). The seed is
//!   decrypted into memory for that single command and dropped at exit.
//! - `wallet lock` removes the marker; spending is refused immediately.
//!
//! This is deliberately stricter than a seed-caching session: the plaintext
//! seed never touches disk, so a stolen session file grants nothing.
//!
//! # Funding modes
//!
//! Every money-moving path goes through the hardened pipeline:
//! `FundingPlan` → (shielded: `VerifiedPczt`) → `VerifiedTransaction` →
//! journal-integrated broadcaster. There is no legacy bypass.

#![forbid(unsafe_code)]

use anyhow::{bail, Context, Result};
use zalkanes_core::consensus_params::ConsensusParams;
use zalkanes_core::types::Network;
use zalkanes_wallet::{
    broadcast::{broadcast_verified, BroadcastOutcome, ZebraBroadcastClient},
    funding::TipSource,
    keystore::{generate_seed, passphrase, Keystore, KeystoreNetwork, SecretString},
    recovery::record_plan,
    FundContext, FundingPlan, FundingSource, FundingUtxo, Journal, ShieldedFunding, ShieldedWallet,
    SqliteShieldedWallet, TransparentFunding, TxRequest, ZebraCanonicalChainSource,
};

// ── Funding mode ─────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum FundingMode {
    /// Spend transparent UTXOs only. Never uses shielded notes.
    Transparent,
    /// Spend shielded notes only. Never silently falls back to transparent.
    Shielded,
    /// Prefer shielded; fall back to transparent ONLY if no shielded wallet
    /// is configured, or the shielded wallet cannot cover the amount on its
    /// own. Funds from the two pools are never combined in one transaction.
    Auto,
}

impl FundingMode {
    pub fn as_str(self) -> &'static str {
        match self {
            FundingMode::Transparent => "transparent",
            FundingMode::Shielded => "shielded",
            FundingMode::Auto => "auto",
        }
    }
}

// ── Paths / environment ──────────────────────────────────────────────────────

pub fn wallet_dir(network: Network) -> std::path::PathBuf {
    std::env::var("ZALKANES_WALLET_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
            std::path::PathBuf::from(home)
                .join(".zalkanes/wallet")
                .join(network.zebra_name())
        })
}

fn db_path(dir: &std::path::Path) -> std::path::PathBuf {
    dir.join("wallet.sqlite")
}
fn keystore_path(dir: &std::path::Path) -> std::path::PathBuf {
    dir.join("keystore.age")
}
fn journal_path(dir: &std::path::Path) -> std::path::PathBuf {
    dir.join("journal.sqlite")
}
fn session_path(dir: &std::path::Path) -> std::path::PathBuf {
    dir.join("session")
}

fn keystore_network(network: Network) -> KeystoreNetwork {
    match network {
        Network::Mainnet => KeystoreNetwork::Mainnet,
        Network::Testnet => KeystoreNetwork::Testnet,
        Network::Regtest => KeystoreNetwork::Regtest,
    }
}

/// Consensus parameters for the wallet stack.
///
/// MUST NOT collapse regtest onto testnet: regtest activates every network
/// upgrade at height 1, so testnet parameters resolve the wrong consensus
/// branch id and block parsing fails ("coinbase tx's claimed height doesn't
/// match its consensus branch ID"). `ConsensusParams` models all three
/// networks correctly.
fn wallet_params(network: Network) -> ConsensusParams {
    ConsensusParams::for_network(network)
}

/// Read the keystore passphrase: `$ZALKANES_PASSPHRASE` for automation, else
/// an interactive prompt. Never echoed, never logged.
fn read_passphrase(prompt: &str) -> Result<SecretString> {
    if let Ok(p) = std::env::var("ZALKANES_PASSPHRASE") {
        if !p.is_empty() {
            return Ok(passphrase(p));
        }
    }
    let entered = rpassword::prompt_password(prompt)
        .context("reading passphrase (set $ZALKANES_PASSPHRASE for non-interactive use)")?;
    if entered.is_empty() {
        bail!("passphrase must not be empty");
    }
    Ok(passphrase(entered))
}

// ── Armed session (contains NO key material) ─────────────────────────────────

/// Default session lifetime: an armed session expires so a forgotten `unlock`
/// does not leave spending permanently armed.
const SESSION_TTL_SECS: u64 = 900;

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn session_is_armed(dir: &std::path::Path) -> bool {
    let Ok(text) = std::fs::read_to_string(session_path(dir)) else {
        return false;
    };
    text.trim()
        .parse::<u64>()
        .map(|expiry| now_secs() < expiry)
        .unwrap_or(false)
}

fn arm_session(dir: &std::path::Path, ttl: u64) -> Result<u64> {
    let expiry = now_secs() + ttl;
    let path = session_path(dir);
    std::fs::write(&path, expiry.to_string()).context("writing session marker")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(expiry)
}

fn disarm_session(dir: &std::path::Path) {
    let _ = std::fs::remove_file(session_path(dir));
}

// ── Wallet commands ──────────────────────────────────────────────────────────

pub fn wallet_create(network: Network, zebra_url: &str) -> Result<()> {
    let dir = wallet_dir(network);
    std::fs::create_dir_all(&dir).context("creating wallet directory")?;
    let ks = Keystore::at(keystore_path(&dir));
    if ks.exists() {
        bail!(
            "a keystore already exists at {}; refusing to overwrite a seed",
            ks.path().display()
        );
    }
    let pass = read_passphrase("New wallet passphrase: ")?;
    let confirm = read_passphrase("Confirm passphrase: ")?;
    if age::secrecy::ExposeSecret::expose_secret(&pass)
        != age::secrecy::ExposeSecret::expose_secret(&confirm)
    {
        bail!("passphrases do not match");
    }

    let chain = ZebraCanonicalChainSource::new(zebra_url.to_string(), wallet_params(network))?;
    let seed = generate_seed();
    ks.create(&pass, &seed, keystore_network(network), 0)?;
    let wallet =
        SqliteShieldedWallet::create_new(&db_path(&dir), wallet_params(network), seed, &chain)?;

    println!("wallet created");
    println!("  directory:       {}", dir.display());
    println!("  keystore:        {}", ks.path().display());
    println!("  unified address: {}", wallet.unified_address()?);
    println!("  birthday height: {}", wallet.birthday_height()?);
    println!();
    println!("The seed exists ONLY inside the encrypted keystore. Back up that");
    println!("file and your passphrase: losing either loses the funds.");
    Ok(())
}

pub fn wallet_restore(network: Network, zebra_url: &str, birthday: u32) -> Result<()> {
    let dir = wallet_dir(network);
    std::fs::create_dir_all(&dir).context("creating wallet directory")?;
    let ks = Keystore::at(keystore_path(&dir));
    if !ks.exists() {
        bail!(
            "no keystore at {} — restore expects the encrypted keystore to be \
             placed there first (the seed is never accepted on the command line)",
            ks.path().display()
        );
    }
    let pass = read_passphrase("Wallet passphrase: ")?;
    let seed = ks.unlock(&pass, keystore_network(network))?;
    let chain = ZebraCanonicalChainSource::new(zebra_url.to_string(), wallet_params(network))?;
    let wallet = SqliteShieldedWallet::restore(
        &db_path(&dir),
        wallet_params(network),
        seed.clone_secret(),
        birthday,
        &chain,
    )?;
    println!("wallet restored");
    println!("  unified address: {}", wallet.unified_address()?);
    println!("  birthday height: {}", wallet.birthday_height()?);
    Ok(())
}

fn open_locked(network: Network) -> Result<(std::path::PathBuf, SqliteShieldedWallet)> {
    let dir = wallet_dir(network);
    let db = db_path(&dir);
    if !db.exists() {
        bail!(
            "no wallet at {} — run `zalkanes wallet create` first",
            db.display()
        );
    }
    let wallet = SqliteShieldedWallet::open_locked(&db, wallet_params(network))?;
    Ok((dir, wallet))
}

pub fn wallet_address(network: Network) -> Result<()> {
    let (_, wallet) = open_locked(network)?;
    println!("{}", wallet.unified_address()?);
    Ok(())
}

pub fn wallet_balance(network: Network) -> Result<()> {
    let (_, wallet) = open_locked(network)?;
    let zat = wallet.balance()?;
    println!("{zat} zat ({:.8} ZEC)", zat as f64 / 1e8);
    Ok(())
}

pub fn wallet_status(network: Network, zebra_url: &str) -> Result<()> {
    let (dir, wallet) = open_locked(network)?;
    let ks = Keystore::at(keystore_path(&dir));
    let info = ks.info()?;
    println!("wallet:      {}", dir.display());
    println!("network:     {}", network.zebra_name());
    println!("keystore:    v{} ({})", info.version, info.network.as_str());
    println!(
        "state:       {}",
        if session_is_armed(&dir) {
            "UNLOCKED (armed; passphrase still required per spend)"
        } else {
            "LOCKED (watch-only: scan/address/balance work; spending refused)"
        }
    );
    println!("address:     {}", wallet.unified_address()?);
    println!("birthday:    {}", wallet.birthday_height()?);
    println!("balance:     {} zat", wallet.balance()?);
    println!("notes:       {}", wallet.canonical_note_summary()?);
    println!("retained:    {}", wallet.retained_note_rows()?);

    match ZebraCanonicalChainSource::new(zebra_url.to_string(), wallet_params(network))
        .and_then(|c| c.canonical_tip())
    {
        Ok(tip) => {
            let s = wallet.sync_status(tip)?;
            println!(
                "sync:        wallet {} / zebra {} ({})",
                s.wallet_scan_height,
                s.zebra_tip_height,
                if s.synced { "synced" } else { "NOT synced" }
            );
        }
        Err(e) => println!("sync:        zebra unreachable ({e})"),
    }
    Ok(())
}

pub fn wallet_scan(network: Network, zebra_url: &str) -> Result<()> {
    let (_, wallet) = open_locked(network)?;
    let chain = ZebraCanonicalChainSource::new(zebra_url.to_string(), wallet_params(network))?;
    // Scanning is watch-capable: it needs no spend authorization.
    wallet.scan_to_tip(&chain)?;
    let tip = chain.canonical_tip()?;
    let s = wallet.sync_status(tip)?;
    println!(
        "scanned to {} ({}), balance {} zat",
        s.wallet_scan_height,
        if s.synced { "synced" } else { "NOT synced" },
        wallet.balance()?
    );
    Ok(())
}

pub fn wallet_unlock(network: Network, ttl: Option<u64>) -> Result<()> {
    let dir = wallet_dir(network);
    let ks = Keystore::at(keystore_path(&dir));
    if !ks.exists() {
        bail!("no keystore at {}", ks.path().display());
    }
    // Verify the passphrase actually opens the keystore before arming.
    let pass = read_passphrase("Wallet passphrase: ")?;
    let _seed = ks.unlock(&pass, keystore_network(network))?;
    let expiry = arm_session(&dir, ttl.unwrap_or(SESSION_TTL_SECS))?;
    println!(
        "wallet unlocked (armed until unix {expiry}; the session file holds NO key material —"
    );
    println!("the passphrase is still required for each spending command)");
    Ok(())
}

pub fn wallet_lock(network: Network) -> Result<()> {
    let dir = wallet_dir(network);
    disarm_session(&dir);
    println!("wallet locked: spending is refused until the next unlock");
    Ok(())
}

// ── Funded transaction pipeline ──────────────────────────────────────────────

/// Everything a funded command needs.
pub struct SpendOptions {
    pub network: Network,
    pub zebra_url: String,
    pub funding: FundingMode,
    pub dry_run: bool,
    pub assume_yes: bool,
    pub confirm_mainnet: bool,
}

fn mainnet_guard(opts: &SpendOptions) -> Result<()> {
    // Planning/inspection (--dry-run) moves no money, so it is not gated.
    if opts.network == Network::Mainnet && !opts.confirm_mainnet && !opts.dry_run {
        bail!(
            "refusing a mainnet money-moving command without --confirm-mainnet \
             (--yes does NOT imply mainnet confirmation)"
        );
    }
    Ok(())
}

fn confirm(opts: &SpendOptions, plan: &FundingPlan) -> Result<bool> {
    println!("{}", plan.describe());
    if opts.funding != FundingMode::Transparent {
        println!();
        println!("Privacy boundary: the ZALK message (contract id, opcode, calldata,");
        println!("and deployed code) is ALWAYS public on chain. Shielded funding hides");
        println!("only the SOURCE OF FUNDS, never the contract interaction.");
    }
    if opts.dry_run {
        println!();
        println!("--dry-run: no proof, no signature, no broadcast.");
        return Ok(false);
    }
    if opts.assume_yes {
        return Ok(true);
    }
    print!("Proceed? [y/N] ");
    use std::io::Write;
    std::io::stdout().flush().ok();
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer).ok();
    Ok(matches!(answer.trim(), "y" | "Y" | "yes"))
}

/// Build the funding source for the selected mode.
///
/// `auto` prefers shielded and falls back to transparent only when no
/// shielded wallet exists or it cannot cover the amount alone; the two pools
/// are never combined within one transaction.
fn build_source(
    opts: &SpendOptions,
    required_zat: u64,
    transparent_utxos: impl Fn() -> Result<(zalkanes_tx::SigningKey, Vec<FundingUtxo>)>,
) -> Result<(Box<dyn FundingSource>, &'static str)> {
    let dir = wallet_dir(opts.network);
    let shielded_available = db_path(&dir).exists();

    let make_shielded = || -> Result<Box<dyn FundingSource>> {
        if !session_is_armed(&dir) {
            bail!("wallet is LOCKED: run `zalkanes wallet unlock` before a shielded spend");
        }
        let pass = read_passphrase("Wallet passphrase: ")?;
        let seed =
            Keystore::at(keystore_path(&dir)).unlock(&pass, keystore_network(opts.network))?;
        let wallet =
            SqliteShieldedWallet::open_locked(&db_path(&dir), wallet_params(opts.network))?;
        wallet.unlock_with_seed(&seed.clone_secret())?;

        // Sync + pin the canonical tip before planning.
        let chain =
            ZebraCanonicalChainSource::new(opts.zebra_url.clone(), wallet_params(opts.network))?;
        wallet.scan_to_tip(&chain)?;
        wallet.set_canonical_tip(chain.canonical_tip()?);
        let carrier = crate::signing_key().ok();
        Ok(Box::new(ShieldedFunding::new(Box::new(wallet), carrier)))
    };

    match opts.funding {
        FundingMode::Shielded => {
            if !shielded_available {
                bail!("--funding shielded requires a wallet; run `zalkanes wallet create`");
            }
            Ok((make_shielded()?, "shielded"))
        }
        FundingMode::Transparent => {
            let (key, utxos) = transparent_utxos()?;
            Ok((Box::new(TransparentFunding::new(key, utxos)), "transparent"))
        }
        FundingMode::Auto => {
            if shielded_available {
                let balance =
                    SqliteShieldedWallet::open_locked(&db_path(&dir), wallet_params(opts.network))
                        .and_then(|w| w.balance())
                        .unwrap_or(0);
                if balance >= required_zat && session_is_armed(&dir) {
                    println!("--funding auto: using the shielded wallet");
                    return Ok((make_shielded()?, "shielded"));
                }
                println!(
                    "--funding auto: shielded wallet cannot cover {required_zat} zat \
                     (balance {balance}, armed: {}); falling back to transparent",
                    session_is_armed(&dir)
                );
            } else {
                println!("--funding auto: no shielded wallet configured; using transparent");
            }
            let (key, utxos) = transparent_utxos()?;
            Ok((Box::new(TransparentFunding::new(key, utxos)), "transparent"))
        }
    }
}

/// Plan → (verify) → broadcast through the hardened pipeline. The ONLY
/// production send path in the CLI.
pub fn execute_request(
    opts: &SpendOptions,
    request: TxRequest,
    required_zat: u64,
    transparent_utxos: impl Fn() -> Result<(zalkanes_tx::SigningKey, Vec<FundingUtxo>)>,
) -> Result<Option<[u8; 32]>> {
    mainnet_guard(opts)?;

    let chain =
        ZebraCanonicalChainSource::new(opts.zebra_url.clone(), wallet_params(opts.network))?;
    let (source, used) = build_source(opts, required_zat, transparent_utxos)?;
    let tip = chain.canonical_tip()?;
    let ctx = FundContext::new(opts.network, tip);

    let mut plan = source.plan(&request, &ctx)?;
    println!(
        "funding mode:       {} (requested {})",
        used,
        opts.funding.as_str()
    );
    if !confirm(opts, &plan)? {
        return Ok(None);
    }

    // Durable journal row BEFORE any authorization work.
    let dir = wallet_dir(opts.network);
    std::fs::create_dir_all(&dir).ok();
    let mut journal = Journal::open(&journal_path(&dir))?;
    record_plan(&mut journal, &plan)?;

    plan.prove(&chain)?;
    journal.transition(
        plan.plan_id(),
        zalkanes_wallet::JournalStage::Proving,
        None,
        None,
    )?;
    journal.transition(
        plan.plan_id(),
        zalkanes_wallet::JournalStage::Proven,
        None,
        None,
    )?;
    plan.sign(&chain)?;
    journal.transition(
        plan.plan_id(),
        zalkanes_wallet::JournalStage::Signing,
        None,
        None,
    )?;
    journal.transition(
        plan.plan_id(),
        zalkanes_wallet::JournalStage::Signed,
        None,
        None,
    )?;

    let verified = plan.extract_verified(&chain)?;
    journal.transition(
        plan.plan_id(),
        zalkanes_wallet::JournalStage::Extracted,
        None,
        None,
    )?;
    journal.transition(
        plan.plan_id(),
        zalkanes_wallet::JournalStage::Verified,
        None,
        None,
    )?;

    let transport = ZebraBroadcastClient::new(opts.zebra_url.clone())?;
    let release = NoopRelease;
    let outcome = broadcast_verified(&mut journal, &chain, &transport, &release, &verified)?;

    let mut display = verified.txid();
    display.reverse();
    match outcome {
        BroadcastOutcome::Accepted => {
            println!("broadcast accepted: {}", hex::encode(display));
            Ok(Some(verified.txid()))
        }
        BroadcastOutcome::Rejected(msg) => bail!("broadcast rejected: {msg}"),
        BroadcastOutcome::Unknown(msg) => {
            println!("broadcast outcome UNKNOWN ({msg})");
            println!(
                "txid {} — note reservations are intentionally held;",
                hex::encode(display)
            );
            println!("run `zalkanes wallet status` after the next block to reconcile.");
            Ok(Some(verified.txid()))
        }
    }
}

/// Release hook for transparent plans (which hold no shielded note locks).
struct NoopRelease;
impl zalkanes_wallet::broadcast::ReleaseHook for NoopRelease {
    fn release(&self, _plan_id: &str) -> Result<()> {
        Ok(())
    }
}
