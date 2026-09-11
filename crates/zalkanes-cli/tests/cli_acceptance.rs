//! §16 Priority 4 — CLI acceptance against the real binary.
//!
//! Everything here runs locally with NO funds and NO network: wallet custody
//! lifecycle, lock-state enforcement, dry-run side-effect freedom, mainnet
//! confirmation, and funding-mode plumbing. Live funded CLI runs against a
//! node are recorded in `audit/live-acceptance-evidence.md`.

use std::process::Command;

fn bin() -> std::path::PathBuf {
    // The integration test binary lives next to the built CLI.
    let mut p = std::env::current_exe().unwrap();
    p.pop();
    if p.ends_with("deps") {
        p.pop();
    }
    p.join("zalkanes")
}

struct Env {
    dir: std::path::PathBuf,
}

impl Env {
    fn new(tag: &str) -> Self {
        let mut suffix = [0u8; 8];
        getrandom(&mut suffix);
        let dir = std::env::temp_dir().join(format!("zalkanes-cli-{tag}-{}", hex(&suffix)));
        std::fs::create_dir_all(&dir).unwrap();
        Self { dir }
    }

    fn cmd(&self, args: &[&str]) -> std::process::Output {
        Command::new(bin())
            .args(args)
            .env("ZALKANES_WALLET_DIR", &self.dir)
            .env("ZALKANES_NETWORK", "regtest")
            .env("ZALKANES_ZCASH_RPC_URL", "http://127.0.0.1:1")
            .env("ZALKANES_PASSPHRASE", "test-passphrase")
            .output()
            .expect("run zalkanes")
    }

    fn cmd_env(&self, args: &[&str], extra: &[(&str, &str)]) -> std::process::Output {
        let mut c = Command::new(bin());
        c.args(args)
            .env("ZALKANES_WALLET_DIR", &self.dir)
            .env("ZALKANES_NETWORK", "regtest")
            .env("ZALKANES_ZCASH_RPC_URL", "http://127.0.0.1:1")
            .env("ZALKANES_PASSPHRASE", "test-passphrase");
        for (k, v) in extra {
            c.env(k, v);
        }
        c.output().expect("run zalkanes")
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn getrandom(buf: &mut [u8]) {
    use std::io::Read;
    std::fs::File::open("/dev/urandom")
        .unwrap()
        .read_exact(buf)
        .unwrap();
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn out(o: &std::process::Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

// ── Help / surface ───────────────────────────────────────────────────────────

#[test]
fn wallet_and_funding_surface_exists() {
    let e = Env::new("surface");
    let help = out(&e.cmd(&["wallet", "--help"]));
    for sub in [
        "create", "restore", "address", "balance", "status", "scan", "unlock", "lock",
    ] {
        assert!(help.contains(sub), "wallet help missing `{sub}`:\n{help}");
    }
    for cmd in ["deploy", "call"] {
        let h = out(&e.cmd(&["contract", cmd, "--help"]));
        for flag in [
            "--funding",
            "--dry-run",
            "--yes",
            "--confirm-mainnet",
            "--wait",
        ] {
            assert!(h.contains(flag), "{cmd} help missing `{flag}`:\n{h}");
        }
    }
}

// ── Mainnet confirmation ─────────────────────────────────────────────────────

#[test]
fn mainnet_money_moving_requires_explicit_confirmation() {
    let e = Env::new("mainnet");
    let zero = "0".repeat(64);
    // --yes must NOT imply mainnet confirmation.
    let o = e.cmd_env(
        &["contract", "call", &zero, "1", "--yes"],
        &[("ZALKANES_NETWORK", "mainnet")],
    );
    assert!(!o.status.success(), "mainnet call must be refused");
    let text = out(&o);
    assert!(
        text.contains("--confirm-mainnet"),
        "expected confirmation guard, got:\n{text}"
    );

    // The same command on regtest is not blocked by that guard (it fails
    // later, on the unreachable node — proving the guard is mainnet-specific).
    let o2 = e.cmd(&["contract", "call", &zero, "1", "--yes"]);
    assert!(
        !out(&o2).contains("--confirm-mainnet"),
        "regtest must not require mainnet confirmation"
    );
}

#[test]
fn mainnet_activation_height_remains_unset() {
    assert_eq!(
        zalkanes_core::consensus::MAINNET_ACTIVATION_HEIGHT,
        None,
        "mainnet activation must stay unset in this release"
    );
}

// ── Custody lifecycle through the real binary ────────────────────────────────

#[test]
fn wallet_commands_require_an_existing_wallet() {
    let e = Env::new("nowallet");
    for args in [
        vec!["wallet", "address"],
        vec!["wallet", "balance"],
        vec!["wallet", "status"],
    ] {
        let o = e.cmd(&args);
        assert!(!o.status.success(), "{args:?} must fail without a wallet");
        assert!(
            out(&o).contains("no wallet") || out(&o).contains("wallet create"),
            "{args:?} should point at `wallet create`:\n{}",
            out(&o)
        );
    }
}

#[test]
fn unlock_without_keystore_fails_loudly() {
    let e = Env::new("nokeystore");
    let o = e.cmd(&["wallet", "unlock"]);
    assert!(!o.status.success());
    assert!(out(&o).contains("no keystore"), "got:\n{}", out(&o));
}

#[test]
fn lock_is_idempotent_and_safe_without_a_session() {
    let e = Env::new("lock");
    let o = e.cmd(&["wallet", "lock"]);
    assert!(o.status.success(), "lock must be safe when not armed");
    assert!(out(&o).contains("locked"));
}

#[test]
fn spending_while_locked_is_refused_before_any_network_use() {
    // A shielded spend with no armed session must be refused by the lock
    // check, NOT by a network error — even though the node is unreachable.
    let e = Env::new("lockedspend");
    // Fabricate a wallet DB + keystore presence marker so the lock check is
    // what fires (no real wallet needed: the lock gate precedes wallet open).
    std::fs::write(e.dir.join("wallet.sqlite"), b"").unwrap();
    let zero = "0".repeat(64);
    let o = e.cmd(&[
        "contract",
        "call",
        &zero,
        "1",
        "--funding",
        "shielded",
        "--yes",
    ]);
    assert!(!o.status.success());
    let text = out(&o);
    assert!(
        text.contains("LOCKED") || text.contains("locked"),
        "locked wallet must refuse a shielded spend, got:\n{text}"
    );
}

// ── Dry run ──────────────────────────────────────────────────────────────────

#[test]
fn dry_run_creates_no_wallet_side_effects() {
    let e = Env::new("dryrun");
    let before: Vec<_> = std::fs::read_dir(&e.dir).unwrap().collect();
    assert!(before.is_empty(), "test starts with an empty wallet dir");

    let zero = "0".repeat(64);
    // Dry-run against an unreachable node: whatever happens, it must not
    // create a journal, a session, or any transaction artifact.
    let _ = e.cmd(&["contract", "call", &zero, "1", "--dry-run"]);

    for artifact in ["journal.sqlite", "session"] {
        assert!(
            !e.dir.join(artifact).exists(),
            "--dry-run must not create {artifact}"
        );
    }
}

#[test]
fn dry_run_on_mainnet_is_allowed_without_confirmation() {
    // Planning/inspection is not money-moving, so --dry-run must not demand
    // --confirm-mainnet (it must fail later, on configuration/network).
    let e = Env::new("dryrunmainnet");
    let zero = "0".repeat(64);
    let o = e.cmd_env(
        &["contract", "call", &zero, "1", "--dry-run"],
        &[("ZALKANES_NETWORK", "mainnet")],
    );
    assert!(
        !out(&o).contains("--confirm-mainnet"),
        "dry-run must not be blocked by the mainnet money guard:\n{}",
        out(&o)
    );
}

// ── Funding-mode plumbing ────────────────────────────────────────────────────

#[test]
fn funding_modes_are_accepted_and_rejected_correctly() {
    let e = Env::new("fundingmodes");
    let zero = "0".repeat(64);
    for mode in ["transparent", "shielded", "auto"] {
        let o = e.cmd(&[
            "contract",
            "call",
            &zero,
            "1",
            "--funding",
            mode,
            "--dry-run",
        ]);
        let text = out(&o);
        assert!(
            !text.contains("invalid value"),
            "--funding {mode} must be accepted:\n{text}"
        );
    }
    let bad = e.cmd(&[
        "contract",
        "call",
        &zero,
        "1",
        "--funding",
        "both",
        "--dry-run",
    ]);
    assert!(
        !bad.status.success(),
        "unknown funding mode must be rejected"
    );
}

#[test]
fn shielded_funding_never_silently_falls_back_to_transparent() {
    // With NO shielded wallet configured, `--funding shielded` must fail —
    // never quietly spend transparent funds instead.
    let e = Env::new("nofallback");
    let zero = "0".repeat(64);
    let o = e.cmd(&[
        "contract",
        "call",
        &zero,
        "1",
        "--funding",
        "shielded",
        "--yes",
    ]);
    assert!(!o.status.success());
    let text = out(&o);
    assert!(
        text.contains("requires a wallet") || text.contains("LOCKED") || text.contains("locked"),
        "shielded mode must refuse rather than fall back:\n{text}"
    );
    assert!(
        !text.contains("broadcast accepted"),
        "shielded mode must never broadcast a transparent transaction"
    );
}
