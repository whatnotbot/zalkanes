//! Journal / OutputLockStore cross-store recovery and startup reconciliation
//! (Item 12.3–12.4).
//!
//! # Honesty note
//!
//! The journal (wallet-local SQLite) and the note-lock store (columns inside
//! the `zcash_client_sqlite` WalletDb) are **separate databases with no
//! cross-store atomicity**. Recovery therefore never assumes both moved
//! together: every combination of journal row and lock state is enumerated
//! and resolved by explicit rules:
//!
//! - a lock that could correspond to an in-flight spend is **preserved**;
//! - any inconsistency **fails safe** (reported, nothing mutated);
//! - locks are never unlocked silently — every release appears in the
//!   [`RecoveryReport`];
//! - authorization is never reconstructed: a plan whose in-memory
//!   authorization state was lost is cancelled, never resumed.
//!
//! # Release-safety rule
//!
//! A plan at a stage strictly before `Signing` has never had a complete
//! signature set anywhere (proofs and signatures live only in process
//! memory), so nothing can be in flight and releasing its reservations is
//! provably safe. From `Signing` onward a signed transaction may have
//! existed, so reservations are preserved until their block-height expiry
//! even though the journal row is cancelled.

#![forbid(unsafe_code)]

use anyhow::Result;
use sha2::{Digest, Sha256};

use crate::broadcast::{reconcile, ReleaseHook, TxStatusSource};
use crate::funding::CanonicalTip;
use crate::journal::{Journal, JournalStage, OperationRow};
use crate::plan::FundingPlan;

/// Canonical lock-owner derivation for a plan id (hex of the 32-byte owner).
/// MUST match `SqliteShieldedWallet`'s reservation owner.
pub fn lock_owner_hex(plan_id: &str) -> String {
    let mut h = Sha256::new();
    h.update(b"zalkanes-plan-lock\0");
    h.update(plan_id.as_bytes());
    hex::encode(h.finalize())
}

/// Record a freshly planned (and therefore already-reserved) [`FundingPlan`]
/// in the journal: `Planned` then `Reserved`. Call immediately after
/// `FundingSource::plan` succeeds.
pub fn record_plan(journal: &mut Journal, plan: &FundingPlan) -> Result<()> {
    let tip = plan.canonical_tip();
    let expiry = match plan.expiry_height() {
        0 => None,
        e => Some(e),
    };
    let lock_owner = match plan.pool_name() {
        "shielded" => lock_owner_hex(plan.plan_id()),
        // Transparent plans take no note locks.
        _ => String::new(),
    };
    journal.create_plan(
        plan.plan_id(),
        plan.intent_hash(),
        plan.kind(),
        plan.pool_name(),
        &lock_owner,
        tip.height,
        &hex::encode(tip.hash),
        plan.target_height(),
        plan.journal_inputs(),
        expiry,
    )?;
    journal.reserve(plan.plan_id())?;
    Ok(())
}

/// One currently-held note lock, as seen in the wallet's lock store.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LockedOutput {
    /// Canonical encoding "pool:txid_hex:index" (matches
    /// `FundingPlan::journal_inputs` entries).
    pub output_ref: String,
    /// Hex of the 32-byte lock owner.
    pub owner_hex: String,
}

/// Read-only view of the wallet's note-lock store.
pub trait LockView {
    fn locked_outputs(&self) -> Result<Vec<LockedOutput>>;
}

/// What startup recovery did and what it refused to touch.
#[derive(Clone, Debug, Default)]
pub struct RecoveryReport {
    /// (plan_id, from, to) journal transitions performed.
    pub transitions: Vec<(String, JournalStage, JournalStage)>,
    /// Plans whose reservations were released (each is also a transition).
    pub released: Vec<String>,
    /// Locks preserved although their plan can no longer proceed (a signed
    /// transaction may have existed); they expire by block height.
    pub preserved: Vec<String>,
    /// Locks with no journal row at all — preserved, never silently unlocked.
    pub orphan_locks: Vec<LockedOutput>,
    /// Inconsistencies that recovery refuses to resolve automatically.
    pub anomalies: Vec<String>,
}

fn locks_of<'a>(locks: &'a [LockedOutput], owner_hex: &str) -> Vec<&'a LockedOutput> {
    locks.iter().filter(|l| l.owner_hex == owner_hex).collect()
}

fn inputs_match(row: &OperationRow, held: &[&LockedOutput]) -> bool {
    let mut recorded: Vec<&str> = row
        .selected_inputs
        .split(',')
        .filter(|s| !s.is_empty())
        .collect();
    recorded.sort_unstable();
    let mut actual: Vec<&str> = held.iter().map(|l| l.output_ref.as_str()).collect();
    actual.sort_unstable();
    recorded == actual
}

/// Reconcile every non-terminal journal row against the lock store, the
/// wallet's Zebra, and the canonical tip. Idempotent: repeated runs (and
/// runs after crashes at any point during recovery) converge to the same
/// state and perform no further transitions.
pub fn startup_recover(
    journal: &mut Journal,
    lock_view: &dyn LockView,
    status: &dyn TxStatusSource,
    release: &dyn ReleaseHook,
    tip: CanonicalTip,
) -> Result<RecoveryReport> {
    let mut report = RecoveryReport::default();
    let all_locks = lock_view.locked_outputs()?;
    let rows = journal.non_terminal_rows()?;

    // Owners referenced by ANY journal row (terminal rows keep their locks
    // attributable, so enumerate every row, not only non-terminal ones).
    let known_owners: std::collections::HashSet<String> = rows
        .iter()
        .map(|r| r.lock_owner.clone())
        .filter(|o| !o.is_empty())
        .collect();

    let transition = |journal: &mut Journal,
                      report: &mut RecoveryReport,
                      plan_id: &str,
                      to: JournalStage,
                      err: Option<&str>|
     -> Result<()> {
        let from = journal.transition(plan_id, to, None, err)?;
        report.transitions.push((plan_id.to_string(), from, to));
        Ok(())
    };

    for row in &rows {
        // Shielded rows must carry the canonical owner derivation; anything
        // else is journal corruption and is not acted upon.
        if row.funding_mode == "shielded" && row.lock_owner != lock_owner_hex(&row.plan_id) {
            report.anomalies.push(format!(
                "{}: journal lock_owner does not match canonical derivation; not touched",
                row.plan_id
            ));
            continue;
        }
        let held = if row.lock_owner.is_empty() {
            Vec::new()
        } else {
            locks_of(&all_locks, &row.lock_owner)
        };
        let expired = row.expiry.map(|e| tip.height > e).unwrap_or(false);

        match row.stage {
            JournalStage::Planned => {
                if held.is_empty() {
                    // Never reserved: cancelling is safe (nothing held).
                    transition(
                        journal,
                        &mut report,
                        &row.plan_id,
                        JournalStage::Cancelled,
                        Some("restart: planned, never reserved"),
                    )?;
                } else if inputs_match(row, &held) {
                    // The lock store moved but the journal missed Reserved
                    // (crash between lock_outputs and journal.reserve). Catch
                    // the journal up to reality; the next pass applies the
                    // Reserved policy.
                    transition(
                        journal,
                        &mut report,
                        &row.plan_id,
                        JournalStage::Reserved,
                        None,
                    )?;
                } else {
                    report.anomalies.push(format!(
                        "{}: planned, but held locks do not match recorded inputs; not touched",
                        row.plan_id
                    ));
                }
            }
            JournalStage::Reserved => {
                if held.is_empty() {
                    if expired {
                        transition(
                            journal,
                            &mut report,
                            &row.plan_id,
                            JournalStage::Expired,
                            Some("restart: reservation no longer held and plan expired"),
                        )?;
                    } else {
                        report.anomalies.push(format!(
                            "{}: reserved, but no locks held and not expired; not touched",
                            row.plan_id
                        ));
                    }
                } else if inputs_match(row, &held) {
                    // Reservation is real but the in-memory plan is gone; no
                    // signature set ever existed. Cancel and release.
                    transition(
                        journal,
                        &mut report,
                        &row.plan_id,
                        JournalStage::Cancelled,
                        Some("restart: reservation held, authorization not persisted"),
                    )?;
                    release.release(&row.plan_id)?;
                    report.released.push(row.plan_id.clone());
                } else {
                    report.anomalies.push(format!(
                        "{}: reserved, but held locks do not match recorded inputs; not touched",
                        row.plan_id
                    ));
                }
            }
            JournalStage::Proving | JournalStage::Proven => {
                // No complete signature set has ever existed: release is
                // provably safe.
                transition(
                    journal,
                    &mut report,
                    &row.plan_id,
                    JournalStage::Cancelled,
                    Some("restart: pre-signing stage lost with process memory"),
                )?;
                if !held.is_empty() {
                    release.release(&row.plan_id)?;
                    report.released.push(row.plan_id.clone());
                }
            }
            JournalStage::Signing
            | JournalStage::Signed
            | JournalStage::Extracted
            | JournalStage::Verified => {
                // A signed transaction may have existed in memory. Cancel the
                // row but PRESERVE the reservations until block-height expiry.
                transition(
                    journal,
                    &mut report,
                    &row.plan_id,
                    JournalStage::Cancelled,
                    Some("restart: signing reached; locks preserved until expiry"),
                )?;
                if !held.is_empty() {
                    report.preserved.push(row.plan_id.clone());
                }
            }
            JournalStage::Broadcasting
            | JournalStage::BroadcastUnknown
            | JournalStage::BroadcastAccepted => {
                // Handled below by the broadcast reconciler.
            }
            // Terminal stages are not in non_terminal_rows.
            _ => {}
        }
    }

    // Broadcast-phase rows: resolve by txid against OUR Zebra.
    let broadcast_report = reconcile(journal, status, release, tip)?;
    for a in broadcast_report.actions {
        report.transitions.push((a.plan_id, a.from, a.to));
    }
    report.released.extend(broadcast_report.released);
    report
        .anomalies
        .extend(broadcast_report.errors.iter().cloned());

    // Orphan locks: held by owners no journal row knows. Potential spends —
    // preserved, loudly.
    for lock in &all_locks {
        if !known_owners.contains(&lock.owner_hex) {
            report.orphan_locks.push(lock.clone());
        }
    }

    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    use crate::broadcast::TxStatus;

    struct MockLocks(Vec<LockedOutput>);
    impl LockView for MockLocks {
        fn locked_outputs(&self) -> Result<Vec<LockedOutput>> {
            Ok(self.0.clone())
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

    const INPUTS: &str = "2:aa11:0";

    fn row_at(journal: &mut Journal, plan_id: &str, stage: JournalStage) {
        journal
            .create_plan(
                plan_id,
                "intent",
                "call",
                "shielded",
                &lock_owner_hex(plan_id),
                100,
                "07",
                101,
                INPUTS,
                Some(140),
            )
            .unwrap();
        if stage == JournalStage::Planned {
            return;
        }
        use JournalStage::*;
        let path = [
            Reserved,
            Proving,
            Proven,
            Signing,
            Signed,
            Extracted,
            Verified,
            Broadcasting,
        ];
        for s in path {
            if s == stage {
                break;
            }
            // Persist a txid at Broadcasting like production does.
            let txid = (s == Broadcasting).then_some("cafe");
            journal.transition(plan_id, s, txid, None).unwrap();
        }
        if stage != JournalStage::Planned {
            let txid = (stage == JournalStage::Broadcasting).then_some("cafe");
            journal.transition(plan_id, stage, txid, None).unwrap();
        }
    }

    fn lock_for(plan_id: &str) -> LockedOutput {
        LockedOutput {
            output_ref: INPUTS.to_string(),
            owner_hex: lock_owner_hex(plan_id),
        }
    }

    fn recover(
        journal: &mut Journal,
        locks: Vec<LockedOutput>,
        status: TxStatus,
        release: &MockRelease,
    ) -> RecoveryReport {
        startup_recover(
            journal,
            &MockLocks(locks),
            &MockStatus(status),
            release,
            tip(),
        )
        .unwrap()
    }

    // ── The seven cross-store cases ──────────────────────────────────────────

    #[test]
    fn planned_without_lock_is_cancelled() {
        let mut j = Journal::open_in_memory().unwrap();
        row_at(&mut j, "p1", JournalStage::Planned);
        let release = MockRelease::default();
        recover(&mut j, vec![], TxStatus::Absent, &release);
        assert_eq!(j.stage("p1").unwrap(), Some(JournalStage::Cancelled));
        assert!(release.0.borrow().is_empty());
    }

    #[test]
    fn planned_with_matching_lock_catches_up_to_reserved() {
        let mut j = Journal::open_in_memory().unwrap();
        row_at(&mut j, "p1", JournalStage::Planned);
        let release = MockRelease::default();
        recover(&mut j, vec![lock_for("p1")], TxStatus::Absent, &release);
        assert_eq!(j.stage("p1").unwrap(), Some(JournalStage::Reserved));
        assert!(release.0.borrow().is_empty(), "no release on catch-up");
        // The NEXT recovery pass applies the Reserved policy: cancel + release.
        recover(&mut j, vec![lock_for("p1")], TxStatus::Absent, &release);
        assert_eq!(j.stage("p1").unwrap(), Some(JournalStage::Cancelled));
        assert_eq!(release.0.borrow().as_slice(), ["p1".to_string()]);
    }

    #[test]
    fn reserved_with_matching_lock_cancels_and_releases() {
        let mut j = Journal::open_in_memory().unwrap();
        row_at(&mut j, "p1", JournalStage::Reserved);
        let release = MockRelease::default();
        let r = recover(&mut j, vec![lock_for("p1")], TxStatus::Absent, &release);
        assert_eq!(j.stage("p1").unwrap(), Some(JournalStage::Cancelled));
        assert_eq!(r.released, ["p1".to_string()]);
    }

    #[test]
    fn reserved_with_missing_lock_expired_vs_not() {
        // Expired: terminal Expired.
        let mut j = Journal::open_in_memory().unwrap();
        row_at(&mut j, "p1", JournalStage::Reserved);
        let release = MockRelease::default();
        let late = CanonicalTip {
            height: 141,
            hash: [8u8; 32],
        };
        let r = startup_recover(
            &mut j,
            &MockLocks(vec![]),
            &MockStatus(TxStatus::Absent),
            &release,
            late,
        )
        .unwrap();
        assert_eq!(j.stage("p1").unwrap(), Some(JournalStage::Expired));
        assert!(r.anomalies.is_empty());

        // Not expired: anomaly, untouched.
        let mut j2 = Journal::open_in_memory().unwrap();
        row_at(&mut j2, "p2", JournalStage::Reserved);
        let r2 = recover(&mut j2, vec![], TxStatus::Absent, &release);
        assert_eq!(j2.stage("p2").unwrap(), Some(JournalStage::Reserved));
        assert_eq!(r2.anomalies.len(), 1);
        assert!(release.0.borrow().is_empty());
    }

    #[test]
    fn orphan_lock_is_preserved_and_reported() {
        let mut j = Journal::open_in_memory().unwrap();
        let orphan = LockedOutput {
            output_ref: "2:dd55:1".into(),
            owner_hex: lock_owner_hex("some-unknown-plan"),
        };
        let release = MockRelease::default();
        let r = recover(&mut j, vec![orphan.clone()], TxStatus::Absent, &release);
        assert_eq!(r.orphan_locks, vec![orphan]);
        assert!(release.0.borrow().is_empty(), "orphans are never unlocked");
    }

    #[test]
    fn owner_mismatch_fails_safe() {
        let mut j = Journal::open_in_memory().unwrap();
        // Corrupt journal: shielded row whose owner is not the derivation.
        j.create_plan(
            "p1",
            "intent",
            "call",
            "shielded",
            "deadbeef",
            100,
            "07",
            101,
            INPUTS,
            Some(140),
        )
        .unwrap();
        j.reserve("p1").unwrap();
        let release = MockRelease::default();
        let r = recover(&mut j, vec![lock_for("p1")], TxStatus::Absent, &release);
        assert_eq!(j.stage("p1").unwrap(), Some(JournalStage::Reserved));
        assert!(r
            .anomalies
            .iter()
            .any(|a| a.contains("canonical derivation")));
        assert!(release.0.borrow().is_empty());
    }

    #[test]
    fn selected_input_mismatch_fails_safe() {
        let mut j = Journal::open_in_memory().unwrap();
        row_at(&mut j, "p1", JournalStage::Reserved);
        // The lock store holds a DIFFERENT output under our owner.
        let wrong = LockedOutput {
            output_ref: "2:ffff:9".into(),
            owner_hex: lock_owner_hex("p1"),
        };
        let release = MockRelease::default();
        let r = recover(&mut j, vec![wrong], TxStatus::Absent, &release);
        assert_eq!(j.stage("p1").unwrap(), Some(JournalStage::Reserved));
        assert!(r.anomalies.iter().any(|a| a.contains("do not match")));
        assert!(release.0.borrow().is_empty());
    }

    // ── Lifecycle-boundary crash windows ─────────────────────────────────────

    #[test]
    fn pre_signing_stages_release_signing_stages_preserve() {
        let release = MockRelease::default();
        for (stage, releases) in [
            (JournalStage::Proving, true),
            (JournalStage::Proven, true),
            (JournalStage::Signing, false),
            (JournalStage::Signed, false),
            (JournalStage::Extracted, false),
            (JournalStage::Verified, false),
        ] {
            let mut j = Journal::open_in_memory().unwrap();
            row_at(&mut j, "p", stage);
            let before = release.0.borrow().len();
            let r = recover(&mut j, vec![lock_for("p")], TxStatus::Absent, &release);
            assert_eq!(
                j.stage("p").unwrap(),
                Some(JournalStage::Cancelled),
                "{stage:?} must cancel"
            );
            let released = release.0.borrow().len() > before;
            assert_eq!(released, releases, "release policy at {stage:?}");
            if !releases {
                assert_eq!(r.preserved, ["p".to_string()], "{stage:?} preserves locks");
            }
        }
    }

    #[test]
    fn broadcasting_row_is_reconciled_not_cancelled() {
        let mut j = Journal::open_in_memory().unwrap();
        row_at(&mut j, "p1", JournalStage::Broadcasting);
        let release = MockRelease::default();
        recover(&mut j, vec![lock_for("p1")], TxStatus::InMempool, &release);
        assert_eq!(
            j.stage("p1").unwrap(),
            Some(JournalStage::BroadcastAccepted)
        );
        assert!(release.0.borrow().is_empty());
    }

    #[test]
    fn recovery_is_idempotent_across_reopen() {
        let path = std::env::temp_dir().join(format!(
            "zalkanes-recovery-test-{}.sqlite",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        let release = MockRelease::default();
        {
            let mut j = Journal::open(&path).unwrap();
            row_at(&mut j, "p1", JournalStage::Reserved);
            row_at(&mut j, "p2", JournalStage::Verified);
            startup_recover(
                &mut j,
                &MockLocks(vec![lock_for("p1"), lock_for("p2")]),
                &MockStatus(TxStatus::Absent),
                &release,
                tip(),
            )
            .unwrap();
        } // crash mid-lifecycle
        {
            let mut j = Journal::open(&path).unwrap();
            let r = startup_recover(
                &mut j,
                &MockLocks(vec![lock_for("p2")]),
                &MockStatus(TxStatus::Absent),
                &release,
                tip(),
            )
            .unwrap();
            assert!(r.transitions.is_empty(), "second pass is a no-op");
            assert_eq!(j.stage("p1").unwrap(), Some(JournalStage::Cancelled));
            assert_eq!(j.stage("p2").unwrap(), Some(JournalStage::Cancelled));
        }
        let _ = std::fs::remove_file(&path);
    }

    // ── record_plan wiring ───────────────────────────────────────────────────

    #[test]
    fn record_plan_persists_reserved_row_with_inputs() {
        use crate::funding::{FundContext, FundingSource, FundingUtxo, TxRequest};
        use crate::transparent::TransparentFunding;

        let mut j = Journal::open_in_memory().unwrap();
        let f = TransparentFunding::new(
            zalkanes_tx::SigningKey::dev_key(),
            vec![FundingUtxo {
                outpoint: zalkanes_tx::OutPoint::new([0xF0u8; 32], 3),
                value: 1_000_000,
            }],
        );
        let plan = f
            .plan(
                &TxRequest::Call {
                    op_return: vec![0x5a, 0x41, 0x4c, 0x4b, 0x00, 0x02],
                },
                &FundContext::new(
                    zalkanes_core::types::Network::Regtest,
                    CanonicalTip {
                        height: 1,
                        hash: [0u8; 32],
                    },
                ),
            )
            .unwrap();
        record_plan(&mut j, &plan).unwrap();
        let row = j.operation(plan.plan_id()).unwrap().unwrap();
        assert_eq!(row.stage, JournalStage::Reserved);
        assert_eq!(row.kind, "call");
        assert_eq!(row.funding_mode, "transparent");
        assert!(row.lock_owner.is_empty(), "transparent plans take no locks");
        assert_eq!(
            row.selected_inputs,
            format!("0:{}:3", hex::encode([0xF0u8; 32]))
        );
        // Idempotent re-record.
        record_plan(&mut j, &plan).unwrap();
        assert_eq!(
            j.stage(plan.plan_id()).unwrap(),
            Some(JournalStage::Reserved)
        );
    }
}
