//! PROTOCOL V1 codec vectors (ADR-0008 §18): CALL_V1 commitment/payload
//! encode-decode, auth sighash, and ExternalId derivation.
//!
//! Regenerate with ZALKANES_GENERATE_VECTORS=1; default mode verifies the
//! committed JSON byte-for-byte.

use zalkanes_core::types::{v1_external_account_id, ContractId, Network};
use zalkanes_protocol::v1::{
    call_v1_auth_sighash, call_v1_signing_bytes, decode_call_v1_payload, encode_call_v1_commitment,
    encode_call_v1_payload, sign_call_v1, verify_call_v1_auth, CallV1Commitment, CallV1Payload,
};

const VECTOR_PATH: &str = "../../test-vectors/protocol/call-v1.json";

fn secret(n: u8) -> secp256k1::SecretKey {
    let mut bytes = [0u8; 32];
    bytes[31] = n;
    secp256k1::SecretKey::from_slice(&bytes).unwrap()
}

fn sample_payloads() -> Vec<(String, CallV1Payload)> {
    let prevout = [0x11u8; 36];
    let mut signed = CallV1Payload {
        contract_id: ContractId([0xAB; 32]),
        opcode: 3,
        attached: vec![([0xC1; 32], 1_000_000), ([0xC2; 32], 42)],
        recipient: None,
        input: vec![0xDE, 0xAD, 0xBE, 0xEF],
        auth: None,
    };
    let (pubkey, sig) = sign_call_v1(Network::Regtest, &prevout, &signed, &secret(7));
    signed.auth = Some((pubkey, sig));

    vec![
        (
            "plain-no-auth".into(),
            CallV1Payload {
                contract_id: ContractId([0x01; 32]),
                opcode: 999,
                attached: vec![],
                recipient: None,
                input: vec![],
                auth: None,
            },
        ),
        (
            "recipient-only".into(),
            CallV1Payload {
                contract_id: ContractId([0x02; 32]),
                opcode: 1,
                attached: vec![],
                recipient: Some({
                    let mut r = [0u8; 33];
                    r[1..].copy_from_slice(&[0x33; 32]);
                    r
                }),
                input: vec![0x00; 20],
                auth: None,
            },
        ),
        ("signed-with-attachments".into(), signed),
    ]
}

fn build_vectors() -> serde_json::Value {
    let prevout = [0x11u8; 36];
    let mut payload_vectors = Vec::new();
    for (name, payload) in sample_payloads() {
        let encoded = encode_call_v1_payload(&payload);
        let signing = call_v1_signing_bytes(&payload);
        let sighash = call_v1_auth_sighash(Network::Regtest, &prevout, &signing);
        payload_vectors.push(serde_json::json!({
            "name": name,
            "encoded_hex": hex::encode(&encoded),
            "signing_bytes_hex": hex::encode(&signing),
            "auth_sighash_hex": hex::encode(sighash),
        }));
    }

    let commitment = CallV1Commitment {
        payload_hash: [0x5A; 32],
        payload_length: 12_345,
        carrier_count: 3,
    };
    let account = v1_external_account_id(
        Network::Regtest,
        &secp256k1::PublicKey::from_secret_key(&secp256k1::Secp256k1::new(), &secret(7))
            .serialize(),
    );

    serde_json::json!({
        "schema": "zalkanes-call-v1-vectors",
        "commitment_hex": hex::encode(encode_call_v1_commitment(&commitment)),
        "external_account_key7_regtest_hex": hex::encode(account),
        "payloads": payload_vectors,
    })
}

#[test]
fn call_v1_vectors_match_committed_json() {
    let current = serde_json::to_string_pretty(&build_vectors()).unwrap();
    if std::env::var("ZALKANES_GENERATE_VECTORS").is_ok() {
        std::fs::write(VECTOR_PATH, format!("{current}\n")).unwrap();
        return;
    }
    let committed = std::fs::read_to_string(VECTOR_PATH)
        .expect("committed vectors exist (regenerate with ZALKANES_GENERATE_VECTORS=1)");
    assert_eq!(committed.trim_end(), current, "call-v1 vectors drifted");
}

#[test]
fn call_v1_payloads_round_trip_and_verify() {
    let prevout = [0x11u8; 36];
    for (name, payload) in sample_payloads() {
        let encoded = encode_call_v1_payload(&payload);
        let decoded = decode_call_v1_payload(&encoded).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(decoded, payload, "{name}: round trip");
        if payload.auth.is_some() {
            let account = verify_call_v1_auth(Network::Regtest, &prevout, &decoded)
                .expect("valid auth")
                .expect("account");
            assert_eq!(
                account,
                v1_external_account_id(Network::Regtest, &payload.auth.as_ref().unwrap().0)
            );
            // Any bit flip in the signed region must invalidate the auth.
            let mut tampered = decoded.clone();
            tampered.opcode ^= 1;
            assert!(verify_call_v1_auth(Network::Regtest, &prevout, &tampered).is_err());
            // A different funding prevout must invalidate the auth (replay).
            let other_prevout = [0x12u8; 36];
            assert!(verify_call_v1_auth(Network::Regtest, &other_prevout, &decoded).is_err());
        }
    }
}

#[test]
fn spawned_contract_id_vectors() {
    use zalkanes_core::types::{v1_spawned_contract_id, CodeHash, TxId};
    let path = "../../test-vectors/protocol/spawn-id-v1.json";
    let mut vectors = Vec::new();
    for (network, tx_byte, index, spawner_byte, code_byte) in [
        (Network::Regtest, 0x01u8, 0u16, 0xAAu8, 0xBBu8),
        (Network::Regtest, 0x01, 1, 0xAA, 0xBB),
        (Network::Testnet, 0x01, 0, 0xAA, 0xBB),
        (Network::Regtest, 0x02, 0, 0xCC, 0xDD),
    ] {
        let id = v1_spawned_contract_id(
            network,
            &TxId([tx_byte; 32]),
            index,
            &ContractId([spawner_byte; 32]),
            &CodeHash([code_byte; 32]),
        );
        vectors.push(serde_json::json!({
            "network": format!("{network:?}"),
            "txid_byte": tx_byte,
            "spawn_index": index,
            "spawner_byte": spawner_byte,
            "code_hash_byte": code_byte,
            "contract_id_hex": id.as_hex(),
        }));
    }
    let current = serde_json::to_string_pretty(&serde_json::json!({
        "schema": "zalkanes-spawn-id-v1-vectors",
        "vectors": vectors,
    }))
    .unwrap();
    if std::env::var("ZALKANES_GENERATE_VECTORS").is_ok() {
        std::fs::write(path, format!("{current}\n")).unwrap();
        return;
    }
    let committed = std::fs::read_to_string(path)
        .expect("committed vectors exist (regenerate with ZALKANES_GENERATE_VECTORS=1)");
    assert_eq!(committed.trim_end(), current, "spawn-id vectors drifted");
}
