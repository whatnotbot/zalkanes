//! Durable wallet operation journal (lifecycle metadata only).
//!
//! This is NOT a note database and does not duplicate any `WalletDb` note/tree/
//! account state. `OutputLockStore` remains the canonical input/reservation
//! store; this journal tracks the lifecycle of each operation so crashes and
//! ambiguous broadcasts can be reconciled safely.

#![forbid(unsafe_code)]

use anyhow::{anyhow, bail, Result};
use rusqlite::{params, Connection};

/// The lifecycle stage of a wallet operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum JournalStage {
    Planned,
    Reserved,
    Proving,
    Proven,
    Signing,
    Signed,
    Extracted,
    Verified,
    Broadcasting,
    BroadcastUnknown,
    BroadcastAccepted,
    Mined,
    Cancelled,
    Expired,
    Rejected,
    Reorged,
}

impl JournalStage {
    pub fn as_str(self) -> &'static str {
        match self {
            JournalStage::Planned => "planned",
            JournalStage::Reserved => "reserved",
            JournalStage::Proving => "proving",
            JournalStage::Proven => "proven",
            JournalStage::Signing => "signing",
            JournalStage::Signed => "signed",
            JournalStage::Extracted => "extracted",
            JournalStage::Verified => "verified",
            JournalStage::Broadcasting => "broadcasting",
            JournalStage::BroadcastUnknown => "broadcast_unknown",
            JournalStage::BroadcastAccepted => "broadcast_accepted",
            JournalStage::Mined => "mined",
            JournalStage::Cancelled => "cancelled",
            JournalStage::Expired => "expired",
            JournalStage::Rejected => "rejected",
            JournalStage::Reorged => "reorged",
        }
    }

    /// Whether a transition `from -> to` is allowed. Illegal transitions are
    /// refused (no silent mutation).
    pub fn can_transition(from: Self, to: Self) -> bool {
        use JournalStage::*;
        if from == to {
            return true;
        }
        matches!(
            (from, to),
            (Planned, Reserved)
                | (Planned, Cancelled)
                | (Reserved, Proving)
                | (Reserved, Cancelled)
                | (Reserved, Expired)
                | (Reserved, Reorged)
                | (Proving, Proven)
                | (Proving, Cancelled)
                | (Proven, Signing)
                | (Proven, Cancelled)
                | (Signing, Signed)
                | (Signing, Cancelled)
                | (Signed, Extracted)
                | (Signed, Cancelled)
                | (Signed, Expired)
                | (Signed, Reorged)
                | (Extracted, Verified)
                | (Extracted, Cancelled)
                | (Extracted, Expired)
                | (Extracted, Reorged)
                | (Verified, Broadcasting)
                | (Verified, Cancelled)
                | (Verified, Expired)
                | (Verified, Reorged)
                | (Broadcasting, BroadcastAccepted)
                | (Broadcasting, BroadcastUnknown)
                | (Broadcasting, Rejected)
                | (BroadcastUnknown, BroadcastAccepted)
                | (BroadcastUnknown, Mined)
                | (BroadcastUnknown, Rejected)
                | (BroadcastUnknown, Expired)
                | (BroadcastUnknown, Reorged)
                | (BroadcastAccepted, Mined)
                | (BroadcastAccepted, Reorged)
                | (BroadcastAccepted, Expired)
        )
    }
}

/// A durable operation journal backed by a wallet-local SQLite table. The
/// table is `CREATE TABLE IF NOT EXISTS` so it is created idempotently.
pub struct Journal {
    conn: Connection,
}

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS operations (
    plan_id         TEXT PRIMARY KEY,
    intent_hash     TEXT NOT NULL,
    kind            TEXT NOT NULL,
    funding_mode    TEXT NOT NULL,
    lock_owner      TEXT NOT NULL,
    tip_height      INTEGER NOT NULL,
    tip_hash        TEXT NOT NULL,
    target_height   INTEGER NOT NULL,
    selected_inputs TEXT NOT NULL,
    stage           TEXT NOT NULL,
    txid            TEXT,
    expiry          INTEGER,
    last_error      TEXT,
    created_at      INTEGER NOT NULL,
    updated_at      INTEGER NOT NULL
);
"#;

impl Journal {
    pub fn open(path: &std::path::Path) -> Result<Self> {
        let conn = Connection::open(path).map_err(|e| anyhow!("journal open: {e}"))?;
        conn.execute_batch(SCHEMA)
            .map_err(|e| anyhow!("journal schema: {e}"))?;
        Ok(Self { conn })
    }

    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory().map_err(|e| anyhow!("journal open: {e}"))?;
        conn.execute_batch(SCHEMA)
            .map_err(|e| anyhow!("journal schema: {e}"))?;
        Ok(Self { conn })
    }

    fn now() -> i64 {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0)
    }

    /// Record a new operation at `Planned` (or `Reserved`), transactionally.
    #[allow(clippy::too_many_arguments)]
    pub fn begin(
        &mut self,
        plan_id: &str,
        intent_hash: &str,
        kind: &str,
        funding_mode: &str,
        lock_owner: &str,
        tip_height: u32,
        tip_hash: &str,
        target_height: u32,
        selected_inputs: &str,
    ) -> Result<()> {
        let tx = self
            .conn
            .transaction()
            .map_err(|e| anyhow!("journal tx: {e}"))?;
        let now = Self::now();
        tx.execute(
            "INSERT OR REPLACE INTO operations
             (plan_id, intent_hash, kind, funding_mode, lock_owner, tip_height, tip_hash,
              target_height, selected_inputs, stage, txid, expiry, last_error, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'reserved', NULL, NULL, NULL, ?10, ?10)",
            params![
                plan_id,
                intent_hash,
                kind,
                funding_mode,
                lock_owner,
                tip_height,
                tip_hash,
                target_height,
                selected_inputs,
                now
            ],
        )
        .map_err(|e| anyhow!("journal begin: {e}"))?;
        tx.commit().map_err(|e| anyhow!("journal commit: {e}"))?;
        Ok(())
    }

    /// Atomically transition an operation's stage, enforcing the allowed state
    /// machine. Returns the previous stage.
    pub fn transition(
        &mut self,
        plan_id: &str,
        to: JournalStage,
        txid: Option<&str>,
        last_error: Option<&str>,
    ) -> Result<JournalStage> {
        let tx = self
            .conn
            .transaction()
            .map_err(|e| anyhow!("journal tx: {e}"))?;
        let current: String = tx
            .query_row(
                "SELECT stage FROM operations WHERE plan_id = ?1",
                params![plan_id],
                |row| row.get(0),
            )
            .map_err(|e| anyhow!("journal lookup {plan_id}: {e}"))?;
        let from = parse_stage(&current)?;
        if !JournalStage::can_transition(from, to) {
            bail!(
                "illegal journal transition for {plan_id}: {current} -> {}",
                to.as_str()
            );
        }
        let now = Self::now();
        tx.execute(
            "UPDATE operations SET stage = ?1, txid = COALESCE(?2, txid),
             last_error = COALESCE(?3, last_error), updated_at = ?4 WHERE plan_id = ?5",
            params![to.as_str(), txid, last_error, now, plan_id],
        )
        .map_err(|e| anyhow!("journal transition: {e}"))?;
        tx.commit().map_err(|e| anyhow!("journal commit: {e}"))?;
        Ok(from)
    }

    /// The current stage of an operation, if present.
    pub fn stage(&self, plan_id: &str) -> Result<Option<JournalStage>> {
        let mut stmt = self
            .conn
            .prepare("SELECT stage FROM operations WHERE plan_id = ?1")
            .map_err(|e| anyhow!("journal prepare: {e}"))?;
        let mut rows = stmt
            .query(params![plan_id])
            .map_err(|e| anyhow!("journal query: {e}"))?;
        match rows.next().map_err(|e| anyhow!("journal row: {e}"))? {
            Some(row) => {
                let s: String = row.get(0).map_err(|e| anyhow!("journal get: {e}"))?;
                Ok(Some(parse_stage(&s)?))
            }
            None => Ok(None),
        }
    }

    /// All non-terminal entries (for restart reconciliation).
    pub fn non_terminal(&self) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT plan_id FROM operations WHERE stage NOT IN
                 ('mined','cancelled','expired','rejected','reorged')",
            )
            .map_err(|e| anyhow!("journal prepare: {e}"))?;
        let rows = stmt
            .query_map([], |row| row.get(0))
            .map_err(|e| anyhow!("journal query: {e}"))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(|e| anyhow!("journal row: {e}"))?);
        }
        Ok(out)
    }
}

fn parse_stage(s: &str) -> Result<JournalStage> {
    use JournalStage::*;
    Ok(match s {
        "planned" => Planned,
        "reserved" => Reserved,
        "proving" => Proving,
        "proven" => Proven,
        "signing" => Signing,
        "signed" => Signed,
        "extracted" => Extracted,
        "verified" => Verified,
        "broadcasting" => Broadcasting,
        "broadcast_unknown" => BroadcastUnknown,
        "broadcast_accepted" => BroadcastAccepted,
        "mined" => Mined,
        "cancelled" => Cancelled,
        "expired" => Expired,
        "rejected" => Rejected,
        "reorged" => Reorged,
        other => bail!("unknown journal stage {other:?}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn begin(j: &mut Journal, id: &str) {
        j.begin(
            id, "hash", "call", "shielded", "owner", 100, "aa", 101, "[]",
        )
        .unwrap();
    }

    #[test]
    fn state_machine_allows_forward_flow() {
        let mut j = Journal::open_in_memory().unwrap();
        begin(&mut j, "p1");
        assert_eq!(j.stage("p1").unwrap(), Some(JournalStage::Reserved));
        j.transition("p1", JournalStage::Proving, None, None)
            .unwrap();
        j.transition("p1", JournalStage::Proven, None, None)
            .unwrap();
        j.transition("p1", JournalStage::Signing, None, None)
            .unwrap();
        j.transition("p1", JournalStage::Signed, None, None)
            .unwrap();
        j.transition("p1", JournalStage::Extracted, None, None)
            .unwrap();
        j.transition("p1", JournalStage::Verified, None, None)
            .unwrap();
        j.transition("p1", JournalStage::Broadcasting, None, None)
            .unwrap();
        j.transition("p1", JournalStage::BroadcastUnknown, None, None)
            .unwrap();
        j.transition("p1", JournalStage::BroadcastAccepted, None, None)
            .unwrap();
        j.transition("p1", JournalStage::Mined, Some("txid"), None)
            .unwrap();
    }

    #[test]
    fn illegal_transition_is_rejected() {
        let mut j = Journal::open_in_memory().unwrap();
        begin(&mut j, "p1");
        // Reserved -> Verified is not a legal direct transition.
        assert!(j
            .transition("p1", JournalStage::Verified, None, None)
            .is_err());
        // Still reserved.
        assert_eq!(j.stage("p1").unwrap(), Some(JournalStage::Reserved));
    }

    #[test]
    fn broadcast_unknown_is_a_stable_state() {
        let mut j = Journal::open_in_memory().unwrap();
        begin(&mut j, "p1");
        j.transition("p1", JournalStage::Proving, None, None)
            .unwrap();
        j.transition("p1", JournalStage::Proven, None, None)
            .unwrap();
        j.transition("p1", JournalStage::Signing, None, None)
            .unwrap();
        j.transition("p1", JournalStage::Signed, None, None)
            .unwrap();
        j.transition("p1", JournalStage::Extracted, None, None)
            .unwrap();
        j.transition("p1", JournalStage::Verified, None, None)
            .unwrap();
        j.transition("p1", JournalStage::Broadcasting, None, None)
            .unwrap();
        j.transition("p1", JournalStage::BroadcastUnknown, None, None)
            .unwrap();
        assert_eq!(j.stage("p1").unwrap(), Some(JournalStage::BroadcastUnknown));
        // From unknown, it can resolve to accepted/mined/rejected/expired/reorged.
        j.transition("p1", JournalStage::BroadcastAccepted, None, None)
            .unwrap();
    }

    #[test]
    fn non_terminal_enumerates_open_operations() {
        let mut j = Journal::open_in_memory().unwrap();
        begin(&mut j, "p1");
        begin(&mut j, "p2");
        j.transition("p2", JournalStage::Proving, None, None)
            .unwrap();
        j.transition("p2", JournalStage::Proven, None, None)
            .unwrap();
        j.transition("p2", JournalStage::Signing, None, None)
            .unwrap();
        j.transition("p2", JournalStage::Signed, None, None)
            .unwrap();
        j.transition("p2", JournalStage::Extracted, None, None)
            .unwrap();
        j.transition("p2", JournalStage::Verified, None, None)
            .unwrap();
        j.transition("p2", JournalStage::Broadcasting, None, None)
            .unwrap();
        j.transition("p2", JournalStage::BroadcastAccepted, None, None)
            .unwrap();
        j.transition("p2", JournalStage::Mined, None, None).unwrap();
        let open = j.non_terminal().unwrap();
        assert!(open.contains(&"p1".to_string()));
        assert!(!open.contains(&"p2".to_string()));
    }
}
