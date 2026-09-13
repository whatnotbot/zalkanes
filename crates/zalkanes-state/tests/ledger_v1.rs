//! V1 asset-ledger state coverage (ADR-0008 §2): root integration,
//! rollback, and RocksDB persistence across reopen ("restart equal").

use zalkanes_core::types::{BlockHash, ContractId};
use zalkanes_state::{BlockCommit, MemoryState, RocksState, StateStore};

fn holder(tag: u8, fill: u8) -> [u8; 33] {
    let mut h = [0u8; 33];
    h[0] = tag;
    for b in &mut h[1..] {
        *b = fill;
    }
    h
}

fn commit(height: u32) -> BlockCommit {
    BlockCommit {
        height,
        zcash_block_hash: BlockHash([height as u8; 32]),
        deploys: vec![],
        upserts: vec![],
        deletes: vec![],
        ledger_upserts: vec![],
        ledger_deletes: vec![],
    }
}

fn ledger_scenario<S: StateStore>(mut s: S) {
    let alice = holder(0, 0xaa);
    let pool = holder(1, 0xbb);
    let asset = [0x11u8; 32];

    let empty_root = s.compute_root();

    // Block 1: mint-like upserts.
    let mut c1 = commit(1);
    c1.ledger_upserts.push((alice, asset, 1_000));
    let root1 = s.commit_block(c1).unwrap();
    assert_ne!(root1, empty_root, "ledger rows must move the root");
    assert_eq!(s.ledger_get(&alice, &asset), Some(1_000));

    // Block 2: transfer-like update + a row deletion.
    let mut c2 = commit(2);
    c2.ledger_upserts.push((alice, asset, 400));
    c2.ledger_upserts.push((pool, asset, 600));
    let root2 = s.commit_block(c2).unwrap();
    let mut c3 = commit(3);
    c3.ledger_deletes.push((alice, asset));
    let root3 = s.commit_block(c3).unwrap();
    assert_eq!(s.ledger_get(&alice, &asset), None);
    assert_eq!(s.ledger_get(&pool, &asset), Some(600));

    // Rollback restores every intermediate state exactly.
    s.rollback_to(2).unwrap();
    assert_eq!(s.compute_root(), root2);
    assert_eq!(s.ledger_get(&alice, &asset), Some(400));
    s.rollback_to(1).unwrap();
    assert_eq!(s.compute_root(), root1);
    assert_eq!(s.ledger_get(&pool, &asset), None);
    s.rollback_to(0).unwrap();
    assert_eq!(s.compute_root(), empty_root);
    let _ = root3;
}

#[test]
fn memory_ledger_root_and_rollback() {
    ledger_scenario(MemoryState::new());
}

#[test]
fn rocks_ledger_root_and_rollback() {
    let dir = std::env::temp_dir().join(format!("zalk-ledger-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    ledger_scenario(RocksState::open(&dir).unwrap());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn rocks_ledger_persists_across_reopen() {
    let dir = std::env::temp_dir().join(format!("zalk-ledger-reopen-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let alice = holder(0, 0x77);
    let asset = ContractId([0x22u8; 32]).0;

    let root = {
        let mut s = RocksState::open(&dir).unwrap();
        let mut c = commit(1);
        c.ledger_upserts.push((alice, asset, 123_456));
        s.commit_block(c).unwrap()
    };
    // Reopen: root and ledger row must survive byte-identically.
    let s = RocksState::open(&dir).unwrap();
    assert_eq!(s.compute_root(), root, "restart-equal root");
    assert_eq!(s.ledger_get(&alice, &asset), Some(123_456));
    let _ = std::fs::remove_dir_all(&dir);
}
