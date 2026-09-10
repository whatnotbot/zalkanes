//! Verify the Rust state-root implementation against the independent
//! Python reference implementation's permanent vectors.

use zalkanes_core::types::{BlockHash, CodeHash, ContractId};
use zalkanes_state::{BlockCommit, MemoryState, StateStore};

#[test]
fn state_root_matches_reference_vectors() {
    let vectors: serde_json::Value = serde_json::from_str(include_str!(
        "../../../test-vectors/state-root/vectors.json"
    ))
    .unwrap();
    let arr = vectors.as_array().unwrap();
    assert!(
        arr.len() >= 100,
        "expected >=100 vectors, got {}",
        arr.len()
    );

    for v in arr {
        let n = v["n"].as_u64().unwrap();
        let expected = v["root"].as_str().unwrap();

        // Fixed contract (id = 0x00..0x1f, code_hash = 0xAB*32, 8-byte wasm).
        let cid = ContractId(std::array::from_fn(|i| i as u8));
        let code_hash = CodeHash([0xAB; 32]);
        let wasm = b"\x00asm\x01\x00\x00\x00".to_vec();
        let key = format!("key{n}").into_bytes();
        let value = n.to_be_bytes().to_vec();

        let mut s = MemoryState::new();
        s.commit_block(BlockCommit {
            height: 1,
            zcash_block_hash: BlockHash([0; 32]),
            deploys: vec![(cid, code_hash, wasm)],
            upserts: vec![(cid, key, value)],
            deletes: vec![],
        })
        .unwrap();

        assert_eq!(s.compute_root().as_hex(), expected, "mismatch at n={n}");
    }
}
