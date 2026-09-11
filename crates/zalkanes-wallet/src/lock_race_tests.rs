//! Item 12.5 concurrency: real races against the canonical reservation
//! primitive (`OutputLockStore::lock_outputs` on `zcash_client_sqlite`'s
//! WalletDb), not sequential simulations.
//!
//! The lock is a conditional `UPDATE ... WHERE (not locked or expired or
//! same owner)` inside a SQLite transaction, so exactly one concurrent
//! claimant can win a given output. These tests seed real received-note rows
//! and race two OS threads with separate database connections on the same
//! wallet file.

use std::sync::Barrier;
use zalkanes_core::consensus_params::ConsensusParams;

use rand_core::{OsRng, RngCore};
use secrecy::SecretVec;
use zcash_client_backend::data_api::{
    chain::ChainState,
    locking::{LockOwner, OutputLockStore},
    AccountBirthday, WalletWrite,
};
use zcash_client_backend::wallet::OutputRef;
use zcash_client_sqlite::{util::SystemClock, wallet::init::init_wallet_db, WalletDb};
use zcash_primitives::block::BlockHash;
use zcash_protocol::consensus::BlockHeight;
use zcash_protocol::{PoolType, TxId};

type Db = WalletDb<rusqlite::Connection, ConsensusParams, SystemClock, OsRng>;

fn open_db(path: &std::path::Path) -> Db {
    let mut db = WalletDb::for_path(path, ConsensusParams::Test, SystemClock, OsRng).unwrap();
    init_wallet_db(&mut db, None).unwrap();
    db
}

/// Create a wallet file with one account and `n` seeded ironwood note rows;
/// returns the OutputRefs of the seeded notes.
fn seeded_wallet(tag: &str, n: u32) -> (std::path::PathBuf, Vec<OutputRef>) {
    let mut suffix = [0u8; 8];
    OsRng.fill_bytes(&mut suffix);
    let path = std::env::temp_dir().join(format!(
        "zalkanes-lockrace-{tag}-{}.sqlite",
        hex::encode(suffix)
    ));
    let _ = std::fs::remove_file(&path);

    let mut db = open_db(&path);
    let mut seed = vec![0u8; 32];
    OsRng.fill_bytes(&mut seed);
    let seed = SecretVec::new(seed);
    let birthday = AccountBirthday::from_parts(
        ChainState::empty(BlockHeight::from_u32(100), BlockHash([0u8; 32])),
        None,
    );
    db.create_account("race", &seed, &birthday, None).unwrap();

    // Seed real rows through a separate raw connection (the canonical lock
    // UPDATE operates on these exact tables/columns).
    let conn = rusqlite::Connection::open(&path).unwrap();
    let account_pk: i64 = conn
        .query_row("SELECT id FROM accounts LIMIT 1", [], |r| r.get(0))
        .unwrap();
    let mut refs = Vec::new();
    for i in 0..n {
        let mut txid = [0u8; 32];
        txid[0] = 0xA0 + i as u8;
        conn.execute(
            "INSERT INTO transactions (txid, min_observed_height) VALUES (?1, 100)",
            rusqlite::params![txid.as_slice()],
        )
        .unwrap();
        let id_tx: i64 = conn
            .query_row(
                "SELECT id_tx FROM transactions WHERE txid = ?1",
                rusqlite::params![txid.as_slice()],
                |r| r.get(0),
            )
            .unwrap();
        conn.execute(
            "INSERT INTO ironwood_received_notes
             (transaction_id, action_index, account_id, diversifier, value, rho, rseed,
              is_change, witness_stabilized, note_version)
             VALUES (?1, 0, ?2, ?3, 10000000, ?4, ?5, 0, 0, 3)",
            rusqlite::params![
                id_tx,
                account_pk,
                [0u8; 11].as_slice(),
                [1u8; 32].as_slice(),
                [2u8; 32].as_slice()
            ],
        )
        .unwrap();
        refs.push(OutputRef::new(
            TxId::from_bytes(txid),
            PoolType::IRONWOOD,
            0,
        ));
    }
    (path, refs)
}

fn owner(tag: u8) -> LockOwner {
    LockOwner::new([tag; 32])
}

#[test]
fn same_note_concurrent_lock_exactly_one_wins() {
    let (path, refs) = seeded_wallet("same", 1);
    let target = refs[0];
    let barrier = Barrier::new(2);

    let results: Vec<bool> = std::thread::scope(|s| {
        let handles: Vec<_> = (0u8..2)
            .map(|t| {
                let path = path.clone();
                let barrier = &barrier;
                s.spawn(move || {
                    let mut db = open_db(&path);
                    barrier.wait();
                    db.lock_outputs(&[target], owner(t + 1), BlockHeight::from_u32(300))
                        .is_ok()
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });

    let winners = results.iter().filter(|ok| **ok).count();
    assert_eq!(
        winners, 1,
        "exactly one reservation must succeed: {results:?}"
    );
    let _ = std::fs::remove_file(&path);
}

#[test]
fn disjoint_notes_concurrent_lock_both_proceed() {
    let (path, refs) = seeded_wallet("disjoint", 2);
    let barrier = Barrier::new(2);

    let results: Vec<bool> = std::thread::scope(|s| {
        let handles: Vec<_> = (0u8..2)
            .map(|t| {
                let path = path.clone();
                let barrier = &barrier;
                let target = refs[t as usize];
                s.spawn(move || {
                    let mut db = open_db(&path);
                    barrier.wait();
                    // Disjoint claims may transiently collide on the SQLite
                    // write lock (SQLITE_BUSY); the guarantee is that both
                    // eventually proceed, not that neither ever waits.
                    for _ in 0..50 {
                        if db
                            .lock_outputs(&[target], owner(t + 1), BlockHeight::from_u32(300))
                            .is_ok()
                        {
                            return true;
                        }
                        std::thread::sleep(std::time::Duration::from_millis(10));
                    }
                    false
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });

    assert_eq!(results, vec![true, true], "disjoint notes must both lock");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn reservation_ownership_survives_reopen() {
    let (path, refs) = seeded_wallet("reopen", 1);
    let target = refs[0];
    {
        let mut db = open_db(&path);
        db.lock_outputs(&[target], owner(1), BlockHeight::from_u32(300))
            .unwrap();
    } // process "restart"
    {
        let mut db = open_db(&path);
        // A different owner still cannot take or release the note.
        assert!(
            db.lock_outputs(&[target], owner(2), BlockHeight::from_u32(300))
                .is_err(),
            "foreign owner must not steal a held reservation across restart"
        );
        assert!(
            !db.unlock_output(&target, owner(2)).unwrap(),
            "foreign owner must not release a held reservation"
        );
        // The original owner still can (idempotent re-lock, then release).
        db.lock_outputs(&[target], owner(1), BlockHeight::from_u32(300))
            .unwrap();
        assert!(db.unlock_output(&target, owner(1)).unwrap());
    }
    let _ = std::fs::remove_file(&path);
}

#[test]
fn expired_lock_is_reclaimable_unexpired_is_not() {
    let (path, refs) = seeded_wallet("expiry", 1);
    let target = refs[0];
    let mut db = open_db(&path);
    db.lock_outputs(&[target], owner(1), BlockHeight::from_u32(300))
        .unwrap();

    // No chain tip is recorded in this seeded wallet, so expiry cannot be
    // judged and the conservative rule applies: the foreign owner is refused.
    assert!(db
        .lock_outputs(&[target], owner(2), BlockHeight::from_u32(300))
        .is_err());

    // Record a chain tip beyond the expiry (scan_queue is the canonical
    // "known chain tip" source for lock-expiry judgement): the lock is then
    // expired and the foreign owner may reclaim the output.
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute(
            "INSERT INTO scan_queue (block_range_start, block_range_end, priority)
             VALUES (400, 401, 10)",
            [],
        )
        .unwrap();
    }
    if let Err(e) = db.lock_outputs(&[target], owner(2), BlockHeight::from_u32(500)) {
        panic!("reclaim failed: {e:?}");
    }
    let _ = std::fs::remove_file(&path);
}
