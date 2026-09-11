//! Tamper matrix (transparent half): exact one-field mutation tests proving
//! that [`crate::plan::FundingPlan`] final verification rejects each mutation
//! at its *intended* invariant (asserted via the error message), never as a
//! mere parse failure.
//!
//! ZIP-244 v5 txids exclude scriptSigs, so several of these mutations (chunk
//! bytes, redeem script, chunk index, pubkey) leave the txid unchanged — they
//! are exactly the class of tampering only content verification can catch.
//!
//! The shielded half of the matrix (PCZT pool/spend/nullifier/anchor/change/
//! fee mutations) lives with the `VerifiedPczt` boundary tests behind the
//! `shielded` feature.

use zalkanes_core::types::{CodeHash, Network};
use zalkanes_tx::{OutPoint, SignedTx, SigningKey};

use crate::funding::{CanonicalTip, FundContext, FundingSource, FundingUtxo, TxRequest};
use crate::plan::FundingPlan;
use crate::transparent::TransparentFunding;

struct MockTipSource(CanonicalTip);
impl crate::funding::TipSource for MockTipSource {
    fn canonical_tip(&self) -> anyhow::Result<CanonicalTip> {
        Ok(self.0)
    }
}

fn tip() -> CanonicalTip {
    CanonicalTip {
        height: 1,
        hash: [0u8; 32],
    }
}

fn ctx() -> FundContext {
    FundContext::new(Network::Regtest, tip())
}

fn tip_source() -> MockTipSource {
    MockTipSource(tip())
}

fn outpoint(tag: u8, n: u32) -> OutPoint {
    let mut txid = [0u8; 32];
    txid[0] = tag;
    OutPoint::new(txid, n)
}

fn funding_with(key: SigningKey) -> TransparentFunding {
    TransparentFunding::new(
        key,
        vec![FundingUtxo {
            outpoint: outpoint(0xF0, 0),
            value: 1_000_000,
        }],
    )
}

/// Two distinctive chunks so their bytes can be located in the serialized tx.
fn deploy_chunks() -> Vec<Vec<u8>> {
    vec![vec![0xAA; 100], vec![0xBB; 100]]
}

fn deploy_request(chunks: &[Vec<u8>], carrier_outpoints: Vec<OutPoint>) -> TxRequest {
    let wasm: Vec<u8> = chunks.concat();
    let msg = zalkanes_protocol::DeployMessage {
        code_hash: CodeHash::of(&wasm),
        code_length: wasm.len() as u32,
        chunk_count: chunks.len() as u8,
        output_index: 0,
    };
    TxRequest::Deploy {
        chunks: chunks.to_vec(),
        carrier_outpoints,
        carrier_values: vec![600_000, 500_000],
        op_return: zalkanes_protocol::encode_deploy(&msg),
    }
}

/// Plan → prove → sign → extract, returning the plan and the final tx.
fn extract(f: &TransparentFunding, req: &TxRequest) -> (FundingPlan, SignedTx) {
    let mut plan = f.plan(req, &ctx()).unwrap();
    plan.prove(&tip_source()).unwrap();
    plan.sign(&tip_source()).unwrap();
    let tx = plan.extract(&tip_source()).unwrap();
    (plan, tx)
}

/// Find the unique occurrence of `pattern` in `bytes` (must exist).
fn find(bytes: &[u8], pattern: &[u8]) -> usize {
    bytes
        .windows(pattern.len())
        .position(|w| w == pattern)
        .expect("pattern present in serialized tx")
}

/// Assert `plan.verify_extracted(tx)` fails and its error mentions `marker`
/// (the intended invariant), not merely a parse failure.
fn assert_fails_at(plan: &FundingPlan, tx: &SignedTx, marker: &str) {
    let err = plan
        .verify_extracted(tx)
        .expect_err("mutation must be rejected")
        .to_string();
    assert!(
        err.contains(marker),
        "expected failure at invariant {marker:?}, got: {err}"
    );
}

fn flipped(tx: &SignedTx, at: usize, xor: u8) -> SignedTx {
    let mut bytes = tx.bytes.clone();
    bytes[at] ^= xor;
    SignedTx {
        bytes,
        txid: tx.txid,
    }
}

// ── DEPLOY ───────────────────────────────────────────────────────────────────

#[test]
fn deploy_verifies_clean() {
    let f = funding_with(SigningKey::dev_key());
    let chunks = deploy_chunks();
    let req = deploy_request(&chunks, vec![outpoint(1, 0), outpoint(1, 1)]);
    let (plan, tx) = extract(&f, &req);
    plan.verify_extracted(&tx).unwrap();
}

#[test]
fn deploy_prevout_mutation_rejected() {
    let f = funding_with(SigningKey::dev_key());
    let chunks = deploy_chunks();
    let req_a = deploy_request(&chunks, vec![outpoint(1, 0), outpoint(1, 1)]);
    let req_b = deploy_request(&chunks, vec![outpoint(2, 0), outpoint(1, 1)]);
    let (plan_a, _) = extract(&f, &req_a);
    let (_, tx_b) = extract(&f, &req_b);
    assert_fails_at(&plan_a, &tx_b, "prevout mismatch");
}

#[test]
fn deploy_input_order_mutation_rejected() {
    let f = funding_with(SigningKey::dev_key());
    let chunks = deploy_chunks();
    let req_a = deploy_request(&chunks, vec![outpoint(1, 0), outpoint(1, 1)]);
    // Same outpoint set, reversed order (chunks stay aligned with positions).
    let req_b = deploy_request(&chunks, vec![outpoint(1, 1), outpoint(1, 0)]);
    let (plan_a, _) = extract(&f, &req_a);
    let (_, tx_b) = extract(&f, &req_b);
    assert_fails_at(&plan_a, &tx_b, "prevout mismatch");
}

#[test]
fn deploy_scriptsig_chunk_byte_mutation_rejected() {
    let f = funding_with(SigningKey::dev_key());
    let req = deploy_request(&deploy_chunks(), vec![outpoint(1, 0), outpoint(1, 1)]);
    let (plan, tx) = extract(&f, &req);
    // Flip one byte inside chunk 0's 0xAA run. The txid does NOT change
    // (scriptSigs are outside the ZIP-244 txid), which is the attack.
    let at = find(&tx.bytes, &[0xAA; 20]) + 10;
    assert_fails_at(
        &plan,
        &flipped(&tx, at, 0x01),
        "carrier chunk bytes mismatch",
    );
}

#[test]
fn deploy_redeem_script_mutation_rejected() {
    let key = SigningKey::dev_key();
    let f = funding_with(key.clone());
    let req = deploy_request(&deploy_chunks(), vec![outpoint(1, 0), outpoint(1, 1)]);
    let (plan, tx) = extract(&f, &req);
    // The 36-byte redeem `0x21 <pubkey> OP_CHECKSIG OP_NOP` appears once per
    // carrier input; flip the trailing OP_NOP of the first occurrence.
    let redeem = zalkanes_tx::redeem_script(&key.compressed_pubkey());
    let at = find(&tx.bytes, &redeem) + redeem.len() - 1;
    assert_fails_at(
        &plan,
        &flipped(&tx, at, 0x01),
        "carrier redeem script mismatch",
    );
}

#[test]
fn deploy_chunk_index_mutation_rejected() {
    let f = funding_with(SigningKey::dev_key());
    let req = deploy_request(&deploy_chunks(), vec![outpoint(1, 0), outpoint(1, 1)]);
    let (plan, tx) = extract(&f, &req);
    // scriptSig starts `PUSH([index]) PUSHDATA1(chunk...)`: for input 0 the
    // byte sequence is 0x01 0x00 0x4c 0x64 0xAA… — flip the index byte to 1.
    let at = find(&tx.bytes, &[0x01, 0x00, 0x4c, 0x64, 0xAA, 0xAA]) + 1;
    assert_fails_at(
        &plan,
        &flipped(&tx, at, 0x01),
        "carrier chunk index mismatch",
    );
}

#[test]
fn deploy_inconsistent_code_hash_fails_reconstruction() {
    // A plan whose DEPLOY message declares a code hash that does not match its
    // chunks must be rejected by the final reconstruction gate.
    let f = funding_with(SigningKey::dev_key());
    let chunks = deploy_chunks();
    let msg = zalkanes_protocol::DeployMessage {
        code_hash: CodeHash::of(b"not the chunks"),
        code_length: 200,
        chunk_count: 2,
        output_index: 0,
    };
    let req = TxRequest::Deploy {
        chunks,
        carrier_outpoints: vec![outpoint(1, 0), outpoint(1, 1)],
        carrier_values: vec![600_000, 500_000],
        op_return: zalkanes_protocol::encode_deploy(&msg),
    };
    let (plan, tx) = extract(&f, &req);
    assert_fails_at(&plan, &tx, "deployment byte stream reconstruction failed");
}

#[test]
fn deploy_zalk_op_return_mutation_rejected() {
    let f = funding_with(SigningKey::dev_key());
    let req = deploy_request(&deploy_chunks(), vec![outpoint(1, 0), outpoint(1, 1)]);
    let (plan, tx) = extract(&f, &req);
    // The OP_RETURN script embeds the ZALK magic (`0x6a PUSH(45) "ZALK"…`);
    // flip the payload byte right after the magic (the version byte).
    let at = find(&tx.bytes, &[0x6a, 0x2d, 0x5a, 0x41, 0x4c, 0x4b]) + 6;
    assert_fails_at(&plan, &flipped(&tx, at, 0x01), "output 0 script mismatch");
}

#[test]
fn deploy_effecting_output_value_mutation_rejected() {
    let f = funding_with(SigningKey::dev_key());
    let req = deploy_request(&deploy_chunks(), vec![outpoint(1, 0), outpoint(1, 1)]);
    let (plan, tx) = extract(&f, &req);
    // The change output's 8-byte LE value immediately precedes its P2PKH
    // script `0x19 0x76 0xa9 0x14 …`; flip the low value byte.
    let at = find(&tx.bytes, &[0x19, 0x76, 0xa9, 0x14]) - 8;
    assert_fails_at(&plan, &flipped(&tx, at, 0x01), "value mismatch");
}

#[test]
fn deploy_expiry_mutation_rejected() {
    let f = funding_with(SigningKey::dev_key());
    let req = deploy_request(&deploy_chunks(), vec![outpoint(1, 0), outpoint(1, 1)]);
    let (plan, tx) = extract(&f, &req);
    // v5 header: version(4) | version_group(4) | branch(4) | lock_time(4) |
    // expiry(4). Transparent plans pin expiry 0; set its low byte.
    assert_fails_at(&plan, &flipped(&tx, 16, 0x01), "expiry mismatch");
}

// ── CALL (P2PKH funding input) ───────────────────────────────────────────────

#[test]
fn call_pubkey_mutation_rejected() {
    let key = SigningKey::dev_key();
    let f = funding_with(key.clone());
    let req = TxRequest::Call {
        op_return: vec![0x5a, 0x41, 0x4c, 0x4b, 0x00, 0x02],
    };
    let (plan, tx) = extract(&f, &req);
    // The compressed pubkey appears once, in the P2PKH scriptSig (outputs hold
    // only its hash160). Flip its last byte; txid is unchanged under ZIP-244.
    let pk = key.compressed_pubkey();
    let at = find(&tx.bytes, &pk) + pk.len() - 1;
    assert_fails_at(&plan, &flipped(&tx, at, 0x01), "pubkey mismatch");
}

// ── PREPARE ──────────────────────────────────────────────────────────────────

#[test]
fn prepare_carrier_count_mutation_rejected() {
    let f = funding_with(SigningKey::dev_key());
    let req_a = TxRequest::Prepare {
        carrier_values: vec![300_000, 200_000],
    };
    let req_b = TxRequest::Prepare {
        carrier_values: vec![300_000, 200_000, 100_000],
    };
    let (plan_a, _) = extract(&f, &req_a);
    let (_, tx_b) = extract(&f, &req_b);
    assert_fails_at(&plan_a, &tx_b, "output count mismatch");
}

#[test]
fn prepare_carrier_order_mutation_rejected() {
    let f = funding_with(SigningKey::dev_key());
    let req_a = TxRequest::Prepare {
        carrier_values: vec![300_000, 200_000],
    };
    let req_b = TxRequest::Prepare {
        carrier_values: vec![200_000, 300_000],
    };
    let (plan_a, _) = extract(&f, &req_a);
    let (_, tx_b) = extract(&f, &req_b);
    assert_fails_at(&plan_a, &tx_b, "value mismatch");
}

#[test]
fn prepare_carrier_value_mutation_rejected() {
    let f = funding_with(SigningKey::dev_key());
    let req_a = TxRequest::Prepare {
        carrier_values: vec![300_000, 200_000],
    };
    let req_b = TxRequest::Prepare {
        carrier_values: vec![300_001, 200_000],
    };
    let (plan_a, _) = extract(&f, &req_a);
    let (_, tx_b) = extract(&f, &req_b);
    assert_fails_at(&plan_a, &tx_b, "value mismatch");
}

#[test]
fn prepare_carrier_script_mutation_rejected() {
    // A different carrier key changes the P2SH script hash: the carrier
    // scriptPubKey no longer matches the plan.
    let f_a = funding_with(SigningKey::dev_key());
    let f_b = funding_with(SigningKey::from_secret_bytes([0x22; 32]).unwrap());
    let req = TxRequest::Prepare {
        carrier_values: vec![300_000, 200_000],
    };
    let (plan_a, _) = extract(&f_a, &req);
    let (_, tx_b) = extract(&f_b, &req);
    assert_fails_at(&plan_a, &tx_b, "script mismatch");
}

// ── Freshness (stale pre-broadcast) ──────────────────────────────────────────

#[test]
fn stale_extract_verified_rejected() {
    let f = funding_with(SigningKey::dev_key());
    let req = TxRequest::Call {
        op_return: vec![0x5a, 0x41, 0x4c, 0x4b, 0x00, 0x02],
    };
    let mut plan = f.plan(&req, &ctx()).unwrap();
    plan.prove(&tip_source()).unwrap();
    plan.sign(&tip_source()).unwrap();
    // The chain moves between sign and the pre-broadcast extraction.
    let stale = MockTipSource(CanonicalTip {
        height: 2,
        hash: [0x55; 32],
    });
    match plan.extract_verified(&stale) {
        Ok(_) => panic!("stale extract_verified must fail"),
        Err(err) => assert!(err.to_string().contains("StalePlan"), "got: {err}"),
    }
}
