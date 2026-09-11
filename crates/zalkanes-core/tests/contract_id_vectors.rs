//! Canonical ContractId derivation vectors (cited by ADR-0004). Verify mode
//! checks `test-vectors/protocol/contract-id-v0.json`; generate mode
//! (`ZALKANES_GENERATE_VECTORS=1`) fills in the expected ids.

use zalkanes_core::types::{CodeHash, ContractId, Network, TxId};

const PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../test-vectors/protocol/contract-id-v0.json"
);

fn network(name: &str) -> Network {
    match name {
        "mainnet" => Network::Mainnet,
        "testnet" => Network::Testnet,
        "regtest" => Network::Regtest,
        other => panic!("unknown network {other}"),
    }
}

fn hex32(s: &str) -> [u8; 32] {
    let mut out = [0u8; 32];
    hex::decode_to_slice(s, &mut out).unwrap();
    out
}

#[test]
fn contract_id_vectors_are_reproduced_exactly() {
    let raw = std::fs::read_to_string(PATH).unwrap();
    let mut vectors: Vec<serde_json::Value> = serde_json::from_str(&raw).unwrap();

    let generate = std::env::var("ZALKANES_GENERATE_VECTORS").as_deref() == Ok("1");
    for v in &mut vectors {
        let derived = ContractId::derive(
            network(v["network"].as_str().unwrap()),
            &TxId(hex32(v["txid_hex"].as_str().unwrap())),
            v["output_index"].as_u64().unwrap() as u16,
            &CodeHash(hex32(v["code_hash_hex"].as_str().unwrap())),
        );
        let derived_hex = hex::encode(derived.0);
        if generate {
            v["expected_contract_id_hex"] = serde_json::Value::String(derived_hex);
        } else {
            assert_eq!(
                v["expected_contract_id_hex"].as_str().unwrap(),
                derived_hex,
                "vector {:?} diverged",
                v["description"]
            );
        }
    }
    if generate {
        std::fs::write(PATH, serde_json::to_string_pretty(&vectors).unwrap()).unwrap();
        eprintln!("wrote {} contract-id vectors", vectors.len());
        return;
    }

    // The three committed vectors must be pairwise distinct (index and
    // network scoping).
    let ids: Vec<&str> = vectors
        .iter()
        .map(|v| v["expected_contract_id_hex"].as_str().unwrap())
        .collect();
    assert_ne!(ids[0], ids[1], "output index must scope the id");
    assert_ne!(ids[0], ids[2], "network must scope the id");
}
