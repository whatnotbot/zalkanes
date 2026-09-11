//! Production broadcast + reconciliation (Item 12).
//!
//! # Guarantees
//!
//! - Broadcast accepts **only** [`crate::plan::VerifiedTransaction`] — there
//!   is no raw-bytes production path.
//! - Immediately before `sendrawtransaction` the authoritative [`TipSource`]
//!   (our Zebra) is queried and the current canonical tip must equal the tip
//!   the transaction was verified at; a stale tip fails WITHOUT sending.
//! - The journal row is moved to `Broadcasting` — with the txid persisted —
//!   BEFORE the RPC is issued, so a crash in the send window is always
//!   reconcilable by txid.
//! - Outcomes are classified three ways: explicit acceptance
//!   (`BroadcastAccepted`), explicit deterministic rejection (`Rejected`,
//!   note locks released), and everything ambiguous — timeout, reset, EOF,
//!   malformed response — as `BroadcastUnknown`. Note locks are **never**
//!   released on `BroadcastUnknown`.
//!
//! # Reconciliation
//!
//! [`reconcile`] resolves `Broadcasting` / `BroadcastUnknown` /
//! `BroadcastAccepted` rows against our Zebra only, by txid: mined → `Mined`;
//! in mempool → `BroadcastAccepted`; definitely absent (past expiry by a
//! reorg margin) → `Expired` (locks released); otherwise the row stays
//! ambiguous and its locks stay held.

#![forbid(unsafe_code)]

use anyhow::{anyhow, bail, Result};

use crate::funding::{CanonicalTip, TipSource};
use crate::journal::{Journal, JournalStage};
use crate::plan::VerifiedTransaction;

/// Blocks past `expiry` before an absent transaction is deemed impossible to
/// mine (reorg safety margin).
pub const EXPIRY_ABSENT_MARGIN: u32 = 6;

/// The transport-level result of submitting one raw transaction.
#[derive(Clone, Debug)]
pub enum TransportResult {
    /// The node explicitly accepted the transaction (returned its txid).
    Accepted(String),
    /// The node explicitly and deterministically rejected the transaction.
    Rejected { code: i64, message: String },
    /// Anything else: timeout, connection reset, EOF, malformed response.
    /// The transaction may or may not have reached the node.
    Ambiguous(String),
}

/// Submits raw transactions to OUR Zebra.
pub trait BroadcastTransport {
    fn send_raw_transaction(&self, tx_hex: &str) -> TransportResult;
}

/// The chain-side status of a transaction, per OUR Zebra.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TxStatus {
    Mined { height: u32 },
    InMempool,
    Absent,
}

/// Queries transaction status from OUR Zebra.
pub trait TxStatusSource {
    fn tx_status(&self, txid_hex: &str) -> Result<TxStatus>;
}

/// Releases the note reservations of one plan (deterministic-failure paths
/// only). Blanket-implemented for every [`crate::shielded::ShieldedWallet`].
pub trait ReleaseHook {
    fn release(&self, plan_id: &str) -> Result<()>;
}

impl<T: crate::shielded::ShieldedWallet + ?Sized> ReleaseHook for T {
    fn release(&self, plan_id: &str) -> Result<()> {
        crate::shielded::ShieldedWallet::release(self, plan_id)
    }
}

/// The classified outcome of one production broadcast.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BroadcastOutcome {
    Accepted,
    /// Deterministic rejection; note locks were released.
    Rejected(String),
    /// Ambiguous transport result; note locks remain held.
    Unknown(String),
}

/// Broadcast a verified transaction through the production journal path.
///
/// The journal row for `tx.plan_id()` must be at `Verified`.
pub fn broadcast_verified(
    journal: &mut Journal,
    tip_source: &dyn TipSource,
    transport: &dyn BroadcastTransport,
    release: &dyn ReleaseHook,
    tx: &VerifiedTransaction,
) -> Result<BroadcastOutcome> {
    // Pre-send freshness against the authoritative tip. Stale => do not send.
    let current = tip_source.canonical_tip()?;
    if current != tx.verified_tip() {
        bail!(
            "StalePlan: canonical tip changed since verification (verified {}:{}, current {}:{}); \
             not broadcast — re-plan required",
            tx.verified_tip().height,
            hex::encode(tx.verified_tip().hash),
            current.height,
            hex::encode(current.hash),
        );
    }

    let mut txid_display = tx.txid();
    txid_display.reverse();
    let txid_hex = hex::encode(txid_display);

    // Durably record Broadcasting + txid BEFORE the RPC leaves the process.
    journal.transition(
        tx.plan_id(),
        JournalStage::Broadcasting,
        Some(&txid_hex),
        None,
    )?;

    match transport.send_raw_transaction(&hex::encode(tx.bytes())) {
        TransportResult::Accepted(returned) => {
            if returned != txid_hex {
                // The node accepted *something else*: treat as ambiguous, keep
                // locks, let reconciliation resolve by our txid.
                let msg = format!("accepted txid mismatch: node returned {returned}");
                journal.transition(
                    tx.plan_id(),
                    JournalStage::BroadcastUnknown,
                    None,
                    Some(&msg),
                )?;
                return Ok(BroadcastOutcome::Unknown(msg));
            }
            journal.transition(tx.plan_id(), JournalStage::BroadcastAccepted, None, None)?;
            Ok(BroadcastOutcome::Accepted)
        }
        TransportResult::Rejected { code, message } => {
            let msg = format!("rejected (code {code}): {message}");
            journal.transition(tx.plan_id(), JournalStage::Rejected, None, Some(&msg))?;
            // Deterministic rejection: nothing is in flight; release the notes.
            release.release(tx.plan_id())?;
            Ok(BroadcastOutcome::Rejected(msg))
        }
        TransportResult::Ambiguous(message) => {
            // The transaction may have reached the node. Locks stay held.
            journal.transition(
                tx.plan_id(),
                JournalStage::BroadcastUnknown,
                None,
                Some(&message),
            )?;
            Ok(BroadcastOutcome::Unknown(message))
        }
    }
}

/// One reconciliation action taken on a journal row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReconcileAction {
    pub plan_id: String,
    pub from: JournalStage,
    pub to: JournalStage,
}

/// The result of one reconciliation pass.
#[derive(Clone, Debug, Default)]
pub struct ReconcileReport {
    pub actions: Vec<ReconcileAction>,
    /// Plans whose locks were released (terminal deterministic outcomes only).
    pub released: Vec<String>,
    /// Rows that remain ambiguous (locks intentionally held).
    pub still_unknown: Vec<String>,
    /// Rows that could not be examined (e.g. status query failed).
    pub errors: Vec<String>,
}

/// Reconcile every broadcast-phase journal row against OUR Zebra by txid.
/// Idempotent: a second pass with the same chain state is a no-op.
pub fn reconcile(
    journal: &mut Journal,
    status: &dyn TxStatusSource,
    release: &dyn ReleaseHook,
    tip: CanonicalTip,
) -> Result<ReconcileReport> {
    let mut report = ReconcileReport::default();
    for row in journal.non_terminal_rows()? {
        let stage = row.stage;
        if !matches!(
            stage,
            JournalStage::Broadcasting
                | JournalStage::BroadcastUnknown
                | JournalStage::BroadcastAccepted
        ) {
            continue;
        }
        let Some(txid) = row.txid.clone() else {
            // A broadcast-phase row without a txid cannot be reconciled by
            // txid; keep it ambiguous and loud.
            report
                .errors
                .push(format!("{}: broadcast-phase row has no txid", row.plan_id));
            continue;
        };
        let observed = match status.tx_status(&txid) {
            Ok(s) => s,
            Err(e) => {
                report.errors.push(format!("{}: {e}", row.plan_id));
                continue;
            }
        };
        let mut transition = |journal: &mut Journal, to: JournalStage, err: Option<&str>| {
            journal.transition(&row.plan_id, to, None, err).map(|from| {
                report.actions.push(ReconcileAction {
                    plan_id: row.plan_id.clone(),
                    from,
                    to,
                })
            })
        };
        match observed {
            TxStatus::Mined { .. } => {
                if stage != JournalStage::Mined {
                    transition(journal, JournalStage::Mined, None)?;
                }
            }
            TxStatus::InMempool => {
                if stage != JournalStage::BroadcastAccepted {
                    transition(journal, JournalStage::BroadcastAccepted, None)?;
                }
            }
            TxStatus::Absent => {
                let definitely_expired = row
                    .expiry
                    .map(|e| tip.height > e.saturating_add(EXPIRY_ABSENT_MARGIN))
                    .unwrap_or(false);
                if definitely_expired {
                    // Absent everywhere and past expiry + margin: it can never
                    // be mined. Terminal; release the notes.
                    if stage == JournalStage::Broadcasting {
                        // Route through the ambiguous state (the legal path).
                        transition(journal, JournalStage::BroadcastUnknown, None)?;
                    }
                    transition(
                        journal,
                        JournalStage::Expired,
                        Some("absent past expiry + margin"),
                    )?;
                    release.release(&row.plan_id)?;
                    report.released.push(row.plan_id.clone());
                } else if stage == JournalStage::Broadcasting {
                    // Crash window: Broadcasting persisted, outcome unknown.
                    transition(
                        journal,
                        JournalStage::BroadcastUnknown,
                        Some("absent after restart; send outcome unknown"),
                    )?;
                    report.still_unknown.push(row.plan_id.clone());
                } else if stage == JournalStage::BroadcastUnknown {
                    // Still ambiguous: locks stay held.
                    report.still_unknown.push(row.plan_id.clone());
                } else {
                    // BroadcastAccepted but absent and not yet expired: the
                    // mempool may have dropped it; keep waiting (locks held).
                    report.still_unknown.push(row.plan_id.clone());
                }
            }
        }
    }
    Ok(report)
}

/// Zebra-backed transport + status source (blocking; loopback/private RPC).
pub struct ZebraBroadcastClient {
    url: String,
    client: reqwest::blocking::Client,
}

impl ZebraBroadcastClient {
    pub fn new(url: impl Into<String>) -> Result<Self> {
        Ok(Self {
            url: url.into(),
            client: reqwest::blocking::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .map_err(|e| anyhow!("build http client: {e}"))?,
        })
    }

    fn call(&self, method: &str, params: serde_json::Value) -> Result<serde_json::Value> {
        let body = serde_json::json!({
            "jsonrpc": "1.0", "id": "zalkanes", "method": method, "params": params
        });
        let resp: serde_json::Value = self
            .client
            .post(&self.url)
            .json(&body)
            .send()
            .map_err(|e| anyhow!("transport: {e}"))?
            .json()
            .map_err(|e| anyhow!("malformed response: {e}"))?;
        Ok(resp)
    }
}

impl BroadcastTransport for ZebraBroadcastClient {
    fn send_raw_transaction(&self, tx_hex: &str) -> TransportResult {
        match self.call("sendrawtransaction", serde_json::json!([tx_hex])) {
            Ok(resp) => {
                if let Some(txid) = resp.get("result").and_then(|r| r.as_str()) {
                    return TransportResult::Accepted(txid.to_string());
                }
                match resp.get("error") {
                    Some(err) if !err.is_null() => {
                        let code = err.get("code").and_then(|c| c.as_i64());
                        let message = err
                            .get("message")
                            .and_then(|m| m.as_str())
                            .unwrap_or("")
                            .to_string();
                        match code {
                            // Explicit node-side verification / policy
                            // rejections are deterministic.
                            Some(c @ (-25 | -26 | -27 | -22)) => {
                                TransportResult::Rejected { code: c, message }
                            }
                            // Anything else (server error, unknown code):
                            // ambiguous — the node may have kept the tx.
                            _ => TransportResult::Ambiguous(format!(
                                "unclassified node error {code:?}: {message}"
                            )),
                        }
                    }
                    _ => TransportResult::Ambiguous("empty result and no error".into()),
                }
            }
            Err(e) => TransportResult::Ambiguous(e.to_string()),
        }
    }
}

impl TxStatusSource for ZebraBroadcastClient {
    fn tx_status(&self, txid_hex: &str) -> Result<TxStatus> {
        let resp = self.call("getrawtransaction", serde_json::json!([txid_hex, 1]))?;
        if let Some(result) = resp.get("result").filter(|r| !r.is_null()) {
            let confirmations = result
                .get("confirmations")
                .and_then(|c| c.as_u64())
                .unwrap_or(0);
            if confirmations >= 1 {
                let height = result
                    .get("height")
                    .and_then(|h| h.as_u64())
                    .ok_or_else(|| anyhow!("mined tx without height"))?;
                return Ok(TxStatus::Mined {
                    height: u32::try_from(height).map_err(|_| anyhow!("height out of range"))?,
                });
            }
            return Ok(TxStatus::InMempool);
        }
        match resp.get("error") {
            Some(err) if !err.is_null() => {
                let message = err.get("message").and_then(|m| m.as_str()).unwrap_or("");
                // Zebra / zcashd phrasing for a transaction that is in
                // neither the mempool nor the main chain.
                if message.contains("No such mempool or main chain transaction")
                    || message.contains("No information available about transaction")
                {
                    Ok(TxStatus::Absent)
                } else {
                    Err(anyhow!("getrawtransaction error: {message}"))
                }
            }
            _ => Err(anyhow!("getrawtransaction: empty result and no error")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};

    use crate::plan::VerifiedTransaction;
    use zalkanes_tx::SignedTx;

    struct MockTip(CanonicalTip);
    impl TipSource for MockTip {
        fn canonical_tip(&self) -> Result<CanonicalTip> {
            Ok(self.0)
        }
    }

    struct MockTransport {
        result: TransportResult,
        calls: Cell<usize>,
    }
    impl BroadcastTransport for MockTransport {
        fn send_raw_transaction(&self, _tx_hex: &str) -> TransportResult {
            self.calls.set(self.calls.get() + 1);
            self.result.clone()
        }
    }

    struct MockStatus(TxStatus);
    impl TxStatusSource for MockStatus {
        fn tx_status(&self, _txid: &str) -> Result<TxStatus> {
            Ok(self.0)
        }
    }

    #[derive(Default)]
    struct MockRelease(RefCell<Vec<String>>);
    impl ReleaseHook for MockRelease {
        fn release(&self, plan_id: &str) -> Result<()> {
            self.0.borrow_mut().push(plan_id.to_string());
            Ok(())
        }
    }

    fn tip() -> CanonicalTip {
        CanonicalTip {
            height: 100,
            hash: [7u8; 32],
        }
    }

    /// A fabricated verified transaction whose journal row is at `Verified`.
    fn verified_tx(journal: &mut Journal, plan_id: &str) -> VerifiedTransaction {
        journal
            .create_plan(
                plan_id,
                "intent",
                "call",
                "shielded",
                "owner",
                100,
                "07",
                101,
                "[n1]",
                Some(140),
            )
            .unwrap();
        journal.reserve(plan_id).unwrap();
        for s in [
            JournalStage::Proving,
            JournalStage::Proven,
            JournalStage::Signing,
            JournalStage::Signed,
            JournalStage::Extracted,
            JournalStage::Verified,
        ] {
            journal.transition(plan_id, s, None, None).unwrap();
        }
        let mut txid = [0u8; 32];
        txid[0] = 0xAB;
        VerifiedTransaction::test_new(
            SignedTx {
                bytes: vec![0xCA, 0xFE],
                txid,
            },
            plan_id.to_string(),
            "intent".to_string(),
            tip(),
        )
    }

    fn expected_txid_hex() -> String {
        let mut txid = [0u8; 32];
        txid[0] = 0xAB;
        txid.reverse();
        hex::encode(txid)
    }

    // ── Broadcast classification ─────────────────────────────────────────────

    #[test]
    fn stale_tip_blocks_send_without_state_change() {
        let mut j = Journal::open_in_memory().unwrap();
        let tx = verified_tx(&mut j, "p1");
        let stale_tip = MockTip(CanonicalTip {
            height: 101,
            hash: [9u8; 32],
        });
        let transport = MockTransport {
            result: TransportResult::Accepted(expected_txid_hex()),
            calls: Cell::new(0),
        };
        let release = MockRelease::default();
        let err = broadcast_verified(&mut j, &stale_tip, &transport, &release, &tx)
            .expect_err("stale must fail");
        assert!(err.to_string().contains("StalePlan"), "got: {err}");
        assert_eq!(transport.calls.get(), 0, "must not send on stale tip");
        assert_eq!(j.stage("p1").unwrap(), Some(JournalStage::Verified));
        assert!(release.0.borrow().is_empty());
    }

    #[test]
    fn txid_persisted_at_broadcasting_before_send() {
        let mut j = Journal::open_in_memory().unwrap();
        let tx = verified_tx(&mut j, "p1");
        // The transport result is ambiguous, so the ONLY txid write is the
        // pre-send Broadcasting transition.
        let transport = MockTransport {
            result: TransportResult::Ambiguous("timeout".into()),
            calls: Cell::new(0),
        };
        let release = MockRelease::default();
        let out = broadcast_verified(&mut j, &MockTip(tip()), &transport, &release, &tx).unwrap();
        assert!(matches!(out, BroadcastOutcome::Unknown(_)));
        let row = j.operation("p1").unwrap().unwrap();
        assert_eq!(row.stage, JournalStage::BroadcastUnknown);
        assert_eq!(row.txid.as_deref(), Some(expected_txid_hex().as_str()));
        assert!(release.0.borrow().is_empty(), "no release on Unknown");
    }

    #[test]
    fn accepted_transitions_and_keeps_locks() {
        let mut j = Journal::open_in_memory().unwrap();
        let tx = verified_tx(&mut j, "p1");
        let transport = MockTransport {
            result: TransportResult::Accepted(expected_txid_hex()),
            calls: Cell::new(0),
        };
        let release = MockRelease::default();
        let out = broadcast_verified(&mut j, &MockTip(tip()), &transport, &release, &tx).unwrap();
        assert_eq!(out, BroadcastOutcome::Accepted);
        assert_eq!(
            j.stage("p1").unwrap(),
            Some(JournalStage::BroadcastAccepted)
        );
        assert!(release.0.borrow().is_empty(), "accepted keeps locks");
    }

    #[test]
    fn deterministic_rejection_releases_locks() {
        let mut j = Journal::open_in_memory().unwrap();
        let tx = verified_tx(&mut j, "p1");
        let transport = MockTransport {
            result: TransportResult::Rejected {
                code: -25,
                message: "failed to validate tx".into(),
            },
            calls: Cell::new(0),
        };
        let release = MockRelease::default();
        let out = broadcast_verified(&mut j, &MockTip(tip()), &transport, &release, &tx).unwrap();
        assert!(matches!(out, BroadcastOutcome::Rejected(_)));
        assert_eq!(j.stage("p1").unwrap(), Some(JournalStage::Rejected));
        assert_eq!(release.0.borrow().as_slice(), ["p1".to_string()]);
    }

    #[test]
    fn accepted_with_mismatched_txid_is_ambiguous() {
        let mut j = Journal::open_in_memory().unwrap();
        let tx = verified_tx(&mut j, "p1");
        let transport = MockTransport {
            result: TransportResult::Accepted("beef".into()),
            calls: Cell::new(0),
        };
        let release = MockRelease::default();
        let out = broadcast_verified(&mut j, &MockTip(tip()), &transport, &release, &tx).unwrap();
        assert!(matches!(out, BroadcastOutcome::Unknown(_)));
        assert_eq!(j.stage("p1").unwrap(), Some(JournalStage::BroadcastUnknown));
        assert!(release.0.borrow().is_empty());
    }

    // ── Reconciliation crash windows ─────────────────────────────────────────

    /// Drive a journal row to Broadcasting with a persisted txid (the state a
    /// crash in the send window leaves behind).
    fn crashed_at_broadcasting(journal: &mut Journal, plan_id: &str) {
        let tx = verified_tx(journal, plan_id);
        let _ = tx; // fabricated; only the journal row matters
        journal
            .transition(
                plan_id,
                JournalStage::Broadcasting,
                Some(&expected_txid_hex()),
                None,
            )
            .unwrap();
    }

    #[test]
    fn crash_before_send_converges_to_unknown_then_expired() {
        let mut j = Journal::open_in_memory().unwrap();
        crashed_at_broadcasting(&mut j, "p1");
        let release = MockRelease::default();

        // Restart 1: absent, not yet expired -> BroadcastUnknown, locks held.
        let r = reconcile(&mut j, &MockStatus(TxStatus::Absent), &release, tip()).unwrap();
        assert_eq!(j.stage("p1").unwrap(), Some(JournalStage::BroadcastUnknown));
        assert_eq!(r.still_unknown, ["p1".to_string()]);
        assert!(release.0.borrow().is_empty(), "locks held while ambiguous");

        // Restart 2 (idempotence): same state, no new transitions.
        let r2 = reconcile(&mut j, &MockStatus(TxStatus::Absent), &release, tip()).unwrap();
        assert!(r2.actions.is_empty());

        // Later: absent AND past expiry + margin -> Expired, locks released.
        let late_tip = CanonicalTip {
            height: 140 + EXPIRY_ABSENT_MARGIN + 1,
            hash: [8u8; 32],
        };
        let r3 = reconcile(&mut j, &MockStatus(TxStatus::Absent), &release, late_tip).unwrap();
        assert_eq!(j.stage("p1").unwrap(), Some(JournalStage::Expired));
        assert_eq!(r3.released, ["p1".to_string()]);
        assert_eq!(release.0.borrow().as_slice(), ["p1".to_string()]);
    }

    #[test]
    fn crash_after_send_with_mempool_acceptance_converges() {
        // "Zebra accepted but the journal update was missed."
        let mut j = Journal::open_in_memory().unwrap();
        crashed_at_broadcasting(&mut j, "p1");
        let release = MockRelease::default();
        reconcile(&mut j, &MockStatus(TxStatus::InMempool), &release, tip()).unwrap();
        assert_eq!(
            j.stage("p1").unwrap(),
            Some(JournalStage::BroadcastAccepted)
        );
        assert!(release.0.borrow().is_empty());
    }

    #[test]
    fn mined_before_restart_converges_to_mined() {
        let mut j = Journal::open_in_memory().unwrap();
        crashed_at_broadcasting(&mut j, "p1");
        let release = MockRelease::default();
        reconcile(
            &mut j,
            &MockStatus(TxStatus::Mined { height: 105 }),
            &release,
            tip(),
        )
        .unwrap();
        assert_eq!(j.stage("p1").unwrap(), Some(JournalStage::Mined));
        assert!(release.0.borrow().is_empty());
    }

    #[test]
    fn timeout_then_mempool_resolves_unknown() {
        // RPC timeout after submission, then the tx shows up in the mempool.
        let mut j = Journal::open_in_memory().unwrap();
        let tx = verified_tx(&mut j, "p1");
        let transport = MockTransport {
            result: TransportResult::Ambiguous("read timeout".into()),
            calls: Cell::new(0),
        };
        let release = MockRelease::default();
        broadcast_verified(&mut j, &MockTip(tip()), &transport, &release, &tx).unwrap();
        assert_eq!(j.stage("p1").unwrap(), Some(JournalStage::BroadcastUnknown));

        reconcile(&mut j, &MockStatus(TxStatus::InMempool), &release, tip()).unwrap();
        assert_eq!(
            j.stage("p1").unwrap(),
            Some(JournalStage::BroadcastAccepted)
        );

        reconcile(
            &mut j,
            &MockStatus(TxStatus::Mined { height: 103 }),
            &release,
            tip(),
        )
        .unwrap();
        assert_eq!(j.stage("p1").unwrap(), Some(JournalStage::Mined));
        assert!(
            release.0.borrow().is_empty(),
            "no release on the happy path"
        );
    }

    #[test]
    fn reconciliation_survives_journal_reopen() {
        // Full restart simulation on a real journal file.
        let path = std::env::temp_dir().join(format!(
            "zalkanes-journal-test-{}.sqlite",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        {
            let mut j = Journal::open(&path).unwrap();
            crashed_at_broadcasting(&mut j, "p1");
        } // crash: journal dropped with row at Broadcasting
        let release = MockRelease::default();
        {
            let mut j = Journal::open(&path).unwrap();
            reconcile(&mut j, &MockStatus(TxStatus::Absent), &release, tip()).unwrap();
            assert_eq!(j.stage("p1").unwrap(), Some(JournalStage::BroadcastUnknown));
        } // second crash
        {
            let mut j = Journal::open(&path).unwrap();
            reconcile(&mut j, &MockStatus(TxStatus::InMempool), &release, tip()).unwrap();
            assert_eq!(
                j.stage("p1").unwrap(),
                Some(JournalStage::BroadcastAccepted)
            );
        }
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn broadcast_phase_row_without_txid_is_reported_not_mutated() {
        let mut j = Journal::open_in_memory().unwrap();
        let _tx = verified_tx(&mut j, "p1");
        // Force an inconsistent row: Broadcasting without a txid.
        j.transition("p1", JournalStage::Broadcasting, None, None)
            .unwrap();
        let release = MockRelease::default();
        let r = reconcile(&mut j, &MockStatus(TxStatus::Absent), &release, tip()).unwrap();
        assert_eq!(r.errors.len(), 1);
        assert_eq!(j.stage("p1").unwrap(), Some(JournalStage::Broadcasting));
        assert!(release.0.borrow().is_empty());
    }
}
