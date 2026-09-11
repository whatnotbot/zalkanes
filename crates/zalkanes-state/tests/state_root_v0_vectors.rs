//! Canonical state-root vectors (cited by ADR-0006):
//! `test-vectors/state-roots/v0.json`. The v0 root commits contract leaves
//! and storage leaves only (heights/hashes in the vector file are scenario
//! metadata, not root inputs). Generate mode: `ZALKANES_GENERATE_VECTORS=1`.

use zalkanes_core::types::{BlockHash, ContractId};
use zalkanes_state::{BlockCommit, MemoryState, StateStore};

const PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../test-vectors/state-roots/v0.json"
);

fn hex32(s: &str) -> [u8; 32] {
    let mut out = [0u8; 32];
    hex::decode_to_slice(s, &mut out).unwrap();
    out
}

#[test]
fn state_root_v0_vectors_are_reproduced_exactly() {
    let raw = std::fs::read_to_string(PATH).unwrap();
    let mut vectors: Vec<serde_json::Value> = serde_json::from_str(&raw).unwrap();
    let generate = std::env::var("ZALKANES_GENERATE_VECTORS").as_deref() == Ok("1");

    for v in &mut vectors {
        let mut state = MemoryState::new();
        let upserts = v["state_entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| {
                (
                    ContractId(hex32(e["contract_id"].as_str().unwrap())),
                    hex::decode(e["key_hex"].as_str().unwrap()).unwrap(),
                    hex::decode(e["value_hex"].as_str().unwrap()).unwrap(),
                )
            })
            .collect::<Vec<_>>();
        if !upserts.is_empty() {
            state
                .commit_block(BlockCommit {
                    height: v["height"].as_u64().unwrap() as u32,
                    zcash_block_hash: BlockHash(hex32(v["zcash_block_hash"].as_str().unwrap())),
                    deploys: vec![],
                    upserts,
                    deletes: vec![],
                })
                .unwrap();
        }
        let root_hex = state
            .compute_root()
            .0
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>();
        if generate {
            v["expected_root"] = serde_json::Value::String(root_hex);
        } else {
            assert_eq!(
                v["expected_root"].as_str().unwrap(),
                root_hex,
                "vector {:?} diverged",
                v["description"]
            );
        }
    }
    if generate {
        std::fs::write(PATH, serde_json::to_string_pretty(&vectors).unwrap()).unwrap();
        eprintln!("wrote {} state-root vectors", vectors.len());
    }
}
