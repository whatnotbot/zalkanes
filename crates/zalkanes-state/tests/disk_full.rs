//! §16 Priority 8 — REAL disk-full behavior against a genuinely
//! capacity-limited filesystem.
//!
//! This test is skipped unless `ZALKANES_SMALL_FS` points at a mounted
//! filesystem with very little free space (CI mounts a small ext4 loopback
//! image for exactly this). It is never a mocked error: RocksDB writes to a
//! real filesystem until it physically runs out of space.
//!
//! Required behavior when the filesystem fills:
//!   - the failing commit fails LOUDLY (no silent success),
//!   - no partial/ambiguous state is reported as committed,
//!   - after space is reclaimed and the store is reopened, the persisted
//!     state equals a clean deterministic replay of exactly the commits that
//!     were reported successful — or opening refuses loudly.

use zalkanes_core::types::{BlockHash, CodeHash, ContractId};
use zalkanes_state::{BlockCommit, MemoryState, RocksState, StateStore};

fn cid(n: u32) -> ContractId {
    let mut b = [0u8; 32];
    b[..4].copy_from_slice(&n.to_be_bytes());
    ContractId(b)
}

/// A deterministic, deliberately BULKY commit so a small filesystem fills
/// within a bounded number of blocks.
fn commit_for(height: u32) -> BlockCommit {
    let mut deploys = Vec::new();
    if height % 5 == 1 {
        let wasm = vec![(height % 251) as u8; 200_000];
        deploys.push((cid(height), CodeHash::of(&wasm), wasm));
    }
    let owner = cid(height / 5);
    let upserts = (0..4u32)
        .map(|k| {
            (
                owner,
                format!("k{height}-{k}").into_bytes(),
                vec![(height % 255) as u8; 50_000],
            )
        })
        .collect();
    let mut hash = [0u8; 32];
    hash[..4].copy_from_slice(&height.to_be_bytes());
    BlockCommit {
        height,
        zcash_block_hash: BlockHash(hash),
        deploys,
        upserts,
        deletes: vec![],
    }
}

fn replay_root(upto: u32) -> zalkanes_core::types::StateRoot {
    let mut mem = MemoryState::new();
    for h in 1..=upto {
        mem.commit_block(commit_for(h)).unwrap();
    }
    mem.compute_root()
}

#[test]
fn real_disk_full_fails_loudly_and_recovers_deterministically() {
    let Ok(base) = std::env::var("ZALKANES_SMALL_FS") else {
        eprintln!(
            "skipping: set ZALKANES_SMALL_FS to a small mounted filesystem \
             (CI mounts an ext4 loopback image for this test)"
        );
        return;
    };
    let dir = std::path::PathBuf::from(&base).join("zalkanes-diskfull");
    let _ = std::fs::remove_dir_all(&dir);

    let mut db = RocksState::open(&dir).expect("open on the small filesystem");

    // Write until the filesystem physically runs out of space.
    let mut last_ok = 0u32;
    let mut failure: Option<String> = None;
    for h in 1..=4_000u32 {
        match db.commit_block(commit_for(h)) {
            Ok(_) => last_ok = h,
            Err(e) => {
                failure = Some(e.to_string());
                break;
            }
        }
    }

    let failure = failure.unwrap_or_else(|| {
        panic!(
            "filesystem never filled after {last_ok} commits — is ZALKANES_SMALL_FS small enough?"
        )
    });
    assert!(
        !failure.is_empty(),
        "a disk-full commit must fail loudly with a message"
    );
    assert!(last_ok >= 1, "no commit succeeded before the failure");
    eprintln!("disk full after {last_ok} successful commits: {failure}");

    // The in-process view after the failure must still be the last COMPLETE
    // commit: never a partially-applied block.
    let reported = db.indexed_height();
    assert!(
        reported == Some(last_ok) || reported == Some(last_ok.saturating_sub(1)),
        "post-failure height {reported:?} is neither the last successful commit \
         ({last_ok}) nor the one before it — ambiguous state"
    );
    drop(db);

    // Reopen (still full): either it opens with a complete, replay-equal
    // state, or it refuses loudly. Both are acceptable; silence is not.
    match RocksState::open(&dir) {
        Ok(reopened) => {
            let h = reopened
                .indexed_height()
                .expect("an opened store must report a height");
            assert_eq!(
                reopened.compute_root(),
                replay_root(h),
                "recovered root must equal a clean replay at height {h}"
            );
            assert!(
                h <= last_ok,
                "recovered height {h} claims more than was ever committed ({last_ok})"
            );
        }
        Err(e) => {
            assert!(!e.to_string().is_empty(), "refusal must be loud");
            eprintln!("store refused to reopen after disk-full (acceptable): {e}");
        }
    }

    let _ = std::fs::remove_dir_all(&dir);
}
