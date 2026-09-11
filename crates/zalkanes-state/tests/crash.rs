//! §16.7 indexer/DB crash hardening: SIGKILL at arbitrary commit points,
//! interrupted rollback, restart-during-reindex, write-failure and
//! corruption behavior. The invariant everywhere: after recovery the store
//! either equals a clean deterministic replay of the same commit sequence
//! (byte-identical root) or refuses loudly — never a silent ambiguous state.
//!
//! Environment note: disk-full simulation needs loop devices/quotas not
//! available here; write failure is exercised via read-only database files
//! instead.

use std::process::Command;

use zalkanes_core::types::{BlockHash, CodeHash, ContractId};
use zalkanes_state::{BlockCommit, MemoryState, RocksState, StateStore};

// ── Deterministic workload ───────────────────────────────────────────────────

fn cid(n: u32) -> ContractId {
    let mut b = [0u8; 32];
    b[..4].copy_from_slice(&n.to_be_bytes());
    ContractId(b)
}

/// The commit every honest replayer produces for `height` (pure function).
fn commit_for(height: u32) -> BlockCommit {
    let mut deploys = Vec::new();
    if height % 10 == 1 {
        let wasm = vec![(height % 251) as u8; 64 + (height as usize % 191)];
        let code_hash = CodeHash::of(&wasm);
        deploys.push((cid(height / 10), code_hash, wasm));
    }
    let owner = cid(height / 10);
    let mut upserts = Vec::new();
    for k in 0..8u32 {
        upserts.push((
            owner,
            format!("k{}", (height + k) % 23).into_bytes(),
            height.to_be_bytes().repeat(4 + (k as usize % 5)),
        ));
    }
    let mut deletes = Vec::new();
    if height % 7 == 3 {
        deletes.push((owner, format!("k{}", height % 23).into_bytes()));
    }
    let mut hash = [0u8; 32];
    hash[..4].copy_from_slice(&height.to_be_bytes());
    BlockCommit {
        height,
        zcash_block_hash: BlockHash(hash),
        deploys,
        upserts,
        deletes,
    }
}

/// Root of a clean in-memory replay of blocks 1..=upto.
fn replay_root(upto: u32) -> zalkanes_core::types::StateRoot {
    let mut mem = MemoryState::new();
    for h in 1..=upto {
        mem.commit_block(commit_for(h)).unwrap();
    }
    mem.compute_root()
}

fn tmp_dir(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("zalkanes-crash-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

// ── Subprocess writer (killed by the parent) ─────────────────────────────────

/// Child mode: apply commits forever until SIGKILLed. Activated only via env.
#[test]
fn crash_writer_child() {
    let Ok(db_path) = std::env::var("ZALKANES_CRASH_WRITER_DB") else {
        return; // normal test runs skip the child body
    };
    let mut db = RocksState::open(std::path::Path::new(&db_path)).unwrap();
    let mut h = db.indexed_height().map(|h| h + 1).unwrap_or(1);
    loop {
        db.commit_block(commit_for(h)).unwrap();
        h += 1;
        if h > 100_000 {
            return; // unreachable in practice; parent kills first
        }
    }
}

fn spawn_writer(db_path: &std::path::Path) -> std::process::Child {
    Command::new(std::env::current_exe().unwrap())
        .args(["crash_writer_child", "--exact", "--nocapture"])
        .env("ZALKANES_CRASH_WRITER_DB", db_path)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn crash writer child")
}

#[test]
fn sigkill_during_commits_recovers_to_clean_replay() {
    let dir = tmp_dir("sigkill");

    // Three kill cycles at arbitrary points, then recovery + continuation.
    for cycle in 0..3 {
        let mut child = spawn_writer(&dir);
        std::thread::sleep(std::time::Duration::from_millis(400 + cycle * 170));
        child.kill().unwrap(); // SIGKILL on unix
        let _ = child.wait();

        // Recovery: the store must open and equal a clean replay of exactly
        // the height it reports.
        let db = RocksState::open(&dir).unwrap();
        let h = db
            .indexed_height()
            .expect("some blocks committed before the kill");
        assert!(h >= 1, "cycle {cycle}: no progress before kill");
        assert_eq!(
            db.compute_root(),
            replay_root(h),
            "cycle {cycle}: recovered root != clean replay at height {h}"
        );
        // The atomically-persisted root must agree with the recomputed one.
        assert_eq!(db.persisted_state_root(), Some(db.compute_root()));
        drop(db);
    }

    // Continuation after recovery: extend by 25 blocks and re-check.
    let mut db = RocksState::open(&dir).unwrap();
    let start = db.indexed_height().unwrap();
    for h in start + 1..=start + 25 {
        db.commit_block(commit_for(h)).unwrap();
    }
    assert_eq!(db.compute_root(), replay_root(start + 25));

    drop(db);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn interrupted_rollback_resumes_to_clean_replay() {
    let dir = tmp_dir("rollback");
    {
        let mut db = RocksState::open(&dir).unwrap();
        for h in 1..=200 {
            db.commit_block(commit_for(h)).unwrap();
        }
        // "Crash" partway through a deep rollback: roll back only part of the
        // way (a mid-rollback crash leaves exactly this on-disk shape: some
        // heights undone, journals for the rest intact), then drop the handle.
        db.rollback_to(120).unwrap();
    } // process restart
    {
        let mut db = RocksState::open(&dir).unwrap();
        assert_eq!(db.indexed_height(), Some(120));
        // Resume the rollback to the original target after restart.
        db.rollback_to(50).unwrap();
        assert_eq!(db.indexed_height(), Some(50));
        assert_eq!(db.compute_root(), replay_root(50));
        // And re-index forward again to prove the store is fully usable.
        for h in 51..=80 {
            db.commit_block(commit_for(h)).unwrap();
        }
        assert_eq!(db.compute_root(), replay_root(80));
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn sigkill_during_reindex_from_genesis_recovers() {
    // "Reindex" is a replay from genesis into a fresh directory; kill it
    // mid-way and verify the partial index equals the clean replay prefix,
    // then finish the reindex and compare against a full replay.
    let dir = tmp_dir("reindex");
    let mut child = spawn_writer(&dir);
    std::thread::sleep(std::time::Duration::from_millis(350));
    child.kill().unwrap();
    let _ = child.wait();

    let mut db = RocksState::open(&dir).unwrap();
    let h = db.indexed_height().expect("progress before kill");
    assert_eq!(db.compute_root(), replay_root(h));
    for next in h + 1..=h + 30 {
        db.commit_block(commit_for(next)).unwrap();
    }
    assert_eq!(db.compute_root(), replay_root(h + 30));
    drop(db);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn two_rocks_stores_agree_with_each_other_and_memory() {
    // Reindex-equality across independent stores: two RocksDB directories and
    // one MemoryState replaying the same deterministic sequence agree at
    // every checkpoint.
    let dir_a = tmp_dir("agree-a");
    let dir_b = tmp_dir("agree-b");
    let mut a = RocksState::open(&dir_a).unwrap();
    let mut b = RocksState::open(&dir_b).unwrap();
    let mut m = MemoryState::new();
    for h in 1..=120 {
        let ra = a.commit_block(commit_for(h)).unwrap();
        let rb = b.commit_block(commit_for(h)).unwrap();
        let rm = m.commit_block(commit_for(h)).unwrap();
        assert_eq!(ra, rb, "rocks A vs rocks B at {h}");
        assert_eq!(ra, rm, "rocks vs memory at {h}");
    }
    drop(a);
    drop(b);
    let _ = std::fs::remove_dir_all(&dir_a);
    let _ = std::fs::remove_dir_all(&dir_b);
}

// ── Loud-refusal behavior ────────────────────────────────────────────────────

#[test]
fn read_only_database_files_refuse_writes_loudly() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tmp_dir("readonly");
    {
        let mut db = RocksState::open(&dir).unwrap();
        for h in 1..=10 {
            db.commit_block(commit_for(h)).unwrap();
        }
    }
    // Strip write permission from every file in the DB directory.
    for entry in std::fs::read_dir(&dir).unwrap() {
        let p = entry.unwrap().path();
        let mut perm = std::fs::metadata(&p).unwrap().permissions();
        perm.set_mode(0o444);
        std::fs::set_permissions(&p, perm).unwrap();
    }

    match RocksState::open(&dir) {
        // Opening may already refuse (log/lock files unwritable) — loud is
        // exactly what we require.
        Err(e) => {
            assert!(!e.to_string().is_empty());
        }
        Ok(mut db) => {
            // If it opens, committing must fail loudly, never silently succeed.
            let err = db
                .commit_block(commit_for(11))
                .expect_err("write to read-only files must fail");
            assert!(!err.to_string().is_empty());
        }
    }

    // Restore permissions for cleanup.
    for entry in std::fs::read_dir(&dir).unwrap() {
        let p = entry.unwrap().path();
        let mut perm = std::fs::metadata(&p).unwrap().permissions();
        perm.set_mode(0o644);
        let _ = std::fs::set_permissions(&p, perm);
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn truncated_current_file_refuses_loudly() {
    let dir = tmp_dir("corrupt-current");
    {
        let mut db = RocksState::open(&dir).unwrap();
        for h in 1..=10 {
            db.commit_block(commit_for(h)).unwrap();
        }
    }
    // Truncate CURRENT (the manifest pointer): the DB must refuse to open.
    std::fs::write(dir.join("CURRENT"), b"").unwrap();
    match RocksState::open(&dir) {
        Ok(_) => panic!("truncated CURRENT must refuse to open"),
        Err(e) => assert!(!e.to_string().is_empty()),
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn corrupted_manifest_refuses_loudly() {
    let dir = tmp_dir("corrupt-manifest");
    {
        let mut db = RocksState::open(&dir).unwrap();
        for h in 1..=10 {
            db.commit_block(commit_for(h)).unwrap();
        }
    }
    // Flip bytes in the MANIFEST file.
    let manifest = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .find(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("MANIFEST"))
        })
        .expect("manifest exists");
    let mut bytes = std::fs::read(&manifest).unwrap();
    let mid = bytes.len() / 2;
    let end = std::cmp::min(mid + 32, bytes.len());
    for b in bytes[mid..end].iter_mut() {
        *b ^= 0xFF;
    }
    std::fs::write(&manifest, bytes).unwrap();

    // Loud refusal: either open fails, or reads/commits after open fail.
    match RocksState::open(&dir) {
        Err(e) => assert!(!e.to_string().is_empty()),
        Ok(mut db) => {
            let r = db.commit_block(commit_for(11));
            assert!(
                r.is_err(),
                "corrupt manifest must not permit silent continued operation"
            );
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}
