//! State engine transition invariants under arbitrary commit shapes:
//! commit/rollback round-trips restore the exact prior root, and identical
//! commit sequences are deterministic. No panic, no divergence.

#![no_main]
use libfuzzer_sys::fuzz_target;
use zalkanes_core::types::{BlockHash, CodeHash, ContractId};
use zalkanes_state::{BlockCommit, MemoryState, StateStore};

fn cid(b: u8) -> ContractId {
    ContractId([b; 32])
}

fn commit_from(height: u32, data: &[u8]) -> BlockCommit {
    let mut deploys = Vec::new();
    let mut upserts = Vec::new();
    let mut deletes = Vec::new();
    let mut i = 0usize;
    while i + 4 <= data.len() {
        let op = data[i] % 3;
        let owner = cid(data[i + 1] % 4);
        let klen = (data[i + 2] as usize % 24) + (data[i] as usize % 2); // 0-length keys included
        let end = std::cmp::min(i + 4 + klen, data.len());
        let key = data[i + 4..end].to_vec();
        match op {
            0 => {
                let wasm = vec![data[i + 3]; (data[i + 3] as usize % 64) + 1];
                deploys.push((owner, CodeHash::of(&wasm), wasm));
            }
            1 => upserts.push((owner, key, vec![data[i + 3]; data[i + 3] as usize % 48])),
            _ => deletes.push((owner, key)),
        }
        i = end.max(i + 4);
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

// libfuzzer's default panic hook aborts BEFORE library-internal
// catch_unwind containment (as used by validate_module) can return, so this
// target replaces it with a print-only hook: contained panics behave as in
// production, while target-level assertion failures still unwind into the
// harness and are reported as crashes.
fn print_only_hook() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        std::panic::set_hook(Box::new(|info| eprintln!("{info}")));
    });
}

fuzz_target!(|data: &[u8]| {
    print_only_hook();
    if data.len() < 8 {
        return;
    }
    let (a, b) = data.split_at(data.len() / 2);

    // Determinism: the same sequence in two stores yields identical roots.
    let mut s1 = MemoryState::new();
    let mut s2 = MemoryState::new();
    let c1 = commit_from(1, a);
    let c2 = commit_from(2, b);
    let r1a = s1.commit_block(c1.clone()).unwrap();
    let r1b = s2.commit_block(c1.clone()).unwrap();
    assert_eq!(r1a, r1b, "commit determinism");

    let root_before_2 = s1.compute_root();
    s1.commit_block(c2.clone()).unwrap();
    s2.commit_block(c2).unwrap();
    assert_eq!(s1.compute_root(), s2.compute_root(), "sequence determinism");

    // Rollback restores the exact pre-commit root.
    s1.rollback_to(1).unwrap();
    assert_eq!(s1.compute_root(), root_before_2, "rollback round-trip");
});
