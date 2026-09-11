//! Tamper matrix (shielded half): one-field mutation tests for the
//! [`crate::shielded::VerifiedPczt`] boundary
//! ([`crate::shielded::ShieldedPlan::verify_finalized_pczt`]).
//!
//! Three complementary techniques:
//!
//! 1. **Expectation tweaks** — the boundary compares plan-vs-PCZT
//!    symmetrically, so flipping one recorded expectation against the plan's
//!    own untouched PCZT exercises exactly the same comparison a mutated PCZT
//!    would, deterministically (the builder randomizes action order, so
//!    cross-build index comparisons would be flaky).
//! 2. **Serialized-PCZT byte surgery** — for the 32-byte fields that postcard
//!    serializes verbatim (nullifier, anchor, change recipient), mutate the
//!    actual wire bytes and re-parse: attacker-shaped, end-to-end.
//! 3. **Cross-plan feeds** — a PCZT built for a different tip / payload /
//!    funding pool is fed to the original plan.
//!
//! Every test asserts the error names its intended invariant — never a mere
//! parse failure.
//!
//! Note-proving is deliberately absent: `verify_finalized_pczt` checks
//! plan-binding, while proof validity is enforced by the canonical
//! `TransactionExtractor` (with the verifying key) at extraction. These tests
//! therefore run on unproven builder output, which carries all the fields the
//! boundary verifies.

use anyhow::Result;
use orchard::{
    keys::{FullViewingKey, Scope, SpendAuthorizingKey, SpendingKey},
    note::{ExtractedNoteCommitment, NoteVersion, RandomSeed, Rho},
    tree::{MerkleHashOrchard, MerklePath},
    value::NoteValue,
    Anchor, Note, ValuePool,
};
use zalkanes_core::types::Network;
use zcash_client_backend::{data_api::locking::LockOwner, wallet::OutputRef};

use crate::funding::{CanonicalTip, FundContext, FundingSource, TipSource, TxRequest};
use crate::plan::FundingPlan;
use crate::shielded::{
    ShieldedFunding, ShieldedPlan, ShieldedSelection, ShieldedSpend, ShieldedWallet, SyncStatus,
};

// ── Synthetic notes and a mock wallet ────────────────────────────────────────

const NOTE_VALUE: u64 = 10_000_000;

/// Build a fully valid synthetic spend: real keys, a real note, and a merkle
/// path whose root becomes the (synthetic) anchor. Only proving would reject
/// this tree; building and boundary verification accept it.
fn synthetic_spend(pool: ValuePool, value: u64, seed: u8) -> ShieldedSpend {
    let sk = Option::from(SpendingKey::from_bytes([seed; 32])).expect("valid sk");
    let fvk = FullViewingKey::from(&sk);
    let ask = SpendAuthorizingKey::from(&sk);
    let addr = fvk.address_at(0u32, Scope::External);

    let mut rho_bytes = [0u8; 32];
    rho_bytes[0] = seed;
    let rho = Option::from(Rho::from_bytes(&rho_bytes)).expect("valid rho");
    let mut rseed_bytes = [0u8; 32];
    rseed_bytes[0] = seed ^ 0x5A;
    let rseed = Option::from(RandomSeed::from_bytes(rseed_bytes, &rho)).expect("valid rseed");
    let note_version = match pool {
        ValuePool::Orchard => NoteVersion::V2,
        ValuePool::Ironwood => NoteVersion::V3,
    };
    let note = Option::from(Note::from_parts(
        addr,
        NoteValue::from_raw(value),
        rho,
        rseed,
        note_version,
    ))
    .expect("valid note");

    let leaf = Option::from(MerkleHashOrchard::from_bytes(&[0u8; 32])).expect("valid leaf");
    let merkle_path = MerklePath::from_parts(0, [leaf; 32]);

    ShieldedSpend {
        fvk,
        ask,
        note,
        merkle_path,
        value,
        pool,
    }
}

/// The anchor a synthetic spend's witness commits to.
fn anchor_for(spend: &ShieldedSpend) -> Anchor {
    let cmx: ExtractedNoteCommitment = spend.note.commitment().into();
    spend.merkle_path.root(cmx)
}

struct MockWallet {
    spends: Vec<ShieldedSpend>,
    /// Diversifier index of the (internal-scope) change address.
    change_index: u32,
}

impl MockWallet {
    fn single(pool: ValuePool, seed: u8) -> Self {
        Self {
            spends: vec![synthetic_spend(pool, NOTE_VALUE, seed)],
            change_index: 0,
        }
    }
}

impl ShieldedWallet for MockWallet {
    fn sync_status(&self, tip: CanonicalTip) -> Result<SyncStatus> {
        Ok(SyncStatus {
            zebra_tip_height: tip.height,
            zebra_tip_hash: tip.hash,
            wallet_scan_height: tip.height,
            wallet_scan_hash: tip.hash,
            synced: true,
            anchor_height: Some(tip.height),
        })
    }

    fn select_spends(&self, _required_zat: u64, _plan_id: &str) -> Result<ShieldedSelection> {
        let change_fvk = self.spends[0].fvk.clone();
        let mut orchard_anchor = Some(Anchor::empty_tree());
        let mut ironwood_anchor = Some(Anchor::empty_tree());
        for s in &self.spends {
            match s.pool {
                ValuePool::Orchard => orchard_anchor = Some(anchor_for(s)),
                ValuePool::Ironwood => ironwood_anchor = Some(anchor_for(s)),
            }
        }
        let output_refs = self
            .spends
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let mut txid = [0u8; 32];
                txid[0] = i as u8 + 1;
                OutputRef::new(
                    zcash_protocol::TxId::from_bytes(txid),
                    match s.pool {
                        ValuePool::Orchard => zcash_protocol::PoolType::ORCHARD,
                        ValuePool::Ironwood => zcash_protocol::PoolType::IRONWOOD,
                    },
                    0,
                )
            })
            .collect();
        Ok(ShieldedSelection {
            spends: self.spends.clone(),
            selected_value: self.spends.iter().map(|s| s.value).sum(),
            change_address: change_fvk.address_at(self.change_index, Scope::Internal),
            change_fvk: change_fvk.clone(),
            change_ask: self.spends[0].ask.clone(),
            change_ovk: Some(change_fvk.to_ovk(Scope::Internal)),
            change_pool: ValuePool::Orchard,
            anchor_height: 1,
            orchard_anchor,
            ironwood_anchor,
            output_refs,
            lock_owner: LockOwner::new([0u8; 32]),
        })
    }

    fn release(&self, _plan_id: &str) -> Result<()> {
        Ok(())
    }
}

// ── Plan construction helpers ────────────────────────────────────────────────

fn tip() -> CanonicalTip {
    CanonicalTip {
        height: 1,
        hash: [0u8; 32],
    }
}

fn zalk_call_payload() -> Vec<u8> {
    vec![0x5a, 0x41, 0x4c, 0x4b, 0x00, 0x02, 0xaa, 0xbb]
}

/// Build a planned (unproven) shielded CALL and return its inner ShieldedPlan.
fn shielded_call_plan(wallet: MockWallet, at_tip: CanonicalTip) -> Box<ShieldedPlan> {
    let funding = ShieldedFunding::new(Box::new(wallet), None);
    let req = TxRequest::Call {
        op_return: zalk_call_payload(),
    };
    let ctx = FundContext::new(Network::Regtest, at_tip);
    match funding.plan(&req, &ctx).expect("plan builds") {
        FundingPlan::Shielded(p) => p,
        _ => unreachable!("shielded funding returns shielded plans"),
    }
}

fn default_plan() -> Box<ShieldedPlan> {
    shielded_call_plan(MockWallet::single(ValuePool::Ironwood, 7), tip())
}

/// Serialize the plan's PCZT (consuming it from the plan) for byte surgery.
fn take_pczt_bytes(plan: &mut ShieldedPlan) -> Vec<u8> {
    plan.pczt
        .take()
        .expect("plan has pczt")
        .serialize()
        .expect("pczt serializes")
}

/// Assert verification of `pczt_bytes` against `plan` fails mentioning
/// `marker`, for at least one flipped occurrence of `pattern` (the field may
/// serialize at several positions; the invariant must catch the right one).
fn assert_surgery_fails_at(plan: &ShieldedPlan, pczt_bytes: &[u8], pattern: &[u8], marker: &str) {
    let positions: Vec<usize> = pczt_bytes
        .windows(pattern.len())
        .enumerate()
        .filter(|(_, w)| *w == pattern)
        .map(|(i, _)| i)
        .collect();
    assert!(
        !positions.is_empty(),
        "pattern not found in serialized pczt"
    );
    let mut seen = Vec::new();
    for at in positions {
        let mut mutated = pczt_bytes.to_vec();
        mutated[at] ^= 0x01;
        let Ok(pczt) = pczt::Pczt::parse(&mutated) else {
            continue; // field became unparseable; try the next occurrence
        };
        match plan.verify_finalized_pczt(pczt) {
            // This occurrence of the byte pattern lies in a field the
            // boundary does not bind (e.g. inside a ciphertext); try the next.
            Ok(_) => seen.push(format!("occurrence at {at} ignored")),
            Err(e) => {
                let msg = e.to_string();
                if msg.contains(marker) {
                    return;
                }
                seen.push(msg);
            }
        }
    }
    panic!("no occurrence failed at invariant {marker:?}; saw: {seen:?}");
}

fn reparse(bytes: &[u8]) -> pczt::Pczt {
    pczt::Pczt::parse(bytes).expect("clean pczt reparses")
}

/// Verify and return the error text (avoids Debug on the opaque VerifiedPczt).
fn verify_err(plan: &ShieldedPlan, pczt: pczt::Pczt) -> String {
    match plan.verify_finalized_pczt(pczt) {
        Ok(_) => panic!("verification must fail"),
        Err(e) => e.to_string(),
    }
}

// ── Positive control ─────────────────────────────────────────────────────────

#[test]
fn clean_finalized_pczt_verifies() {
    let mut plan = default_plan();
    let bytes = take_pczt_bytes(&mut plan);
    plan.verify_finalized_pczt(reparse(&bytes))
        .expect("clean pczt verifies against its own plan");
}

// ── Byte surgery: wire-level mutations ───────────────────────────────────────

#[test]
fn pczt_nullifier_mutation_rejected() {
    let mut plan = default_plan();
    let expected_nf = plan.expected.spends[0].nullifier;
    let bytes = take_pczt_bytes(&mut plan);
    assert_surgery_fails_at(&plan, &bytes, &expected_nf, "spend nullifier mismatch");
}

#[test]
fn pczt_anchor_mutation_rejected() {
    let mut plan = default_plan();
    let expected_anchor = plan
        .expected
        .anchor_ironwood
        .expect("ironwood anchor planned");
    let bytes = take_pczt_bytes(&mut plan);
    assert_surgery_fails_at(&plan, &bytes, &expected_anchor, "anchor mismatch");
}

#[test]
fn pczt_change_destination_mutation_rejected() {
    let mut plan = default_plan();
    let expected_addr = plan.expected.change.address_bytes;
    let bytes = take_pczt_bytes(&mut plan);
    // Flip inside the 11-byte diversifier prefix: the address stays parseable
    // but is a different address, so the check (not the parser) must fire.
    let mut pattern = expected_addr.to_vec();
    pattern.truncate(16);
    assert_surgery_fails_at(&plan, &bytes, &pattern, "change destination mismatch");
}

// ── Expectation tweaks: every recorded binding detects a mismatch ────────────

#[test]
fn wrong_nullifier_rejected() {
    let mut plan = default_plan();
    let bytes = take_pczt_bytes(&mut plan);
    plan.expected.spends[0].nullifier[0] ^= 0x01;
    let err = verify_err(&plan, reparse(&bytes));
    assert!(err.contains("spend nullifier mismatch"), "got: {err}");
}

#[test]
fn wrong_spend_value_rejected() {
    let mut plan = default_plan();
    let bytes = take_pczt_bytes(&mut plan);
    plan.expected.spends[0].value += 1;
    let err = verify_err(&plan, reparse(&bytes));
    assert!(err.contains("spend value mismatch"), "got: {err}");
}

#[test]
fn wrong_anchor_rejected() {
    let mut plan = default_plan();
    let bytes = take_pczt_bytes(&mut plan);
    plan.expected.anchor_ironwood.as_mut().unwrap()[0] ^= 0x01;
    let err = verify_err(&plan, reparse(&bytes));
    assert!(err.contains("anchor mismatch"), "got: {err}");
}

#[test]
fn missing_anchor_fails_closed() {
    let mut plan = default_plan();
    let bytes = take_pczt_bytes(&mut plan);
    plan.expected.anchor_ironwood = None;
    let err = verify_err(&plan, reparse(&bytes));
    assert!(err.contains("no planned anchor"), "got: {err}");
}

#[test]
fn wrong_change_value_rejected() {
    let mut plan = default_plan();
    let bytes = take_pczt_bytes(&mut plan);
    plan.expected.change.value += 1;
    let err = verify_err(&plan, reparse(&bytes));
    assert!(err.contains("change value mismatch"), "got: {err}");
}

#[test]
fn wrong_change_destination_rejected() {
    let mut plan = default_plan();
    let bytes = take_pczt_bytes(&mut plan);
    plan.expected.change.address_bytes[2] ^= 0x01;
    let err = verify_err(&plan, reparse(&bytes));
    assert!(err.contains("change destination mismatch"), "got: {err}");
}

#[test]
fn wrong_change_pool_rejected() {
    let mut plan = default_plan();
    let bytes = take_pczt_bytes(&mut plan);
    // Claim the change lives in Ironwood: the Orchard change output becomes
    // unplanned (value-bearing), which is exactly what the boundary rejects.
    plan.expected.change.pool = ValuePool::Ironwood;
    let err = verify_err(&plan, reparse(&bytes));
    assert!(
        err.contains("unplanned value-bearing output") || err.contains("change"),
        "got: {err}"
    );
}

#[test]
fn wrong_action_count_rejected() {
    let mut plan = default_plan();
    let bytes = take_pczt_bytes(&mut plan);
    plan.expected.actions_ironwood += 1;
    let err = verify_err(&plan, reparse(&bytes));
    assert!(err.contains("action count mismatch"), "got: {err}");
}

#[test]
fn wrong_value_balance_rejected() {
    let mut plan = default_plan();
    let bytes = take_pczt_bytes(&mut plan);
    plan.expected.net_ironwood += 1;
    let err = verify_err(&plan, reparse(&bytes));
    assert!(err.contains("value balance mismatch"), "got: {err}");
}

#[test]
fn extra_spend_rejected() {
    // The plan drops its recorded spend: the PCZT's real spend is now an
    // unplanned (value-bearing) spend and must be rejected.
    let mut plan = default_plan();
    let bytes = take_pczt_bytes(&mut plan);
    plan.expected.spends.clear();
    plan.expected.net_ironwood = 0;
    let err = verify_err(&plan, reparse(&bytes));
    assert!(err.contains("extra shielded spend"), "got: {err}");
}

#[test]
fn missing_spend_rejected() {
    // The plan expects a second spend the PCZT does not carry.
    let mut plan = default_plan();
    let bytes = take_pczt_bytes(&mut plan);
    let mut ghost = plan.expected.spends[0].clone();
    ghost.action_index ^= 1; // the other action (a dummy spend)
    ghost.nullifier[1] ^= 0xFF;
    plan.expected.spends.push(ghost);
    let err = verify_err(&plan, reparse(&bytes));
    assert!(err.contains("nullifier mismatch"), "got: {err}");
}

// ── Cross-plan feeds ─────────────────────────────────────────────────────────

#[test]
fn pczt_for_different_tip_rejected_at_expiry() {
    let plan_a = default_plan();
    let other_tip = CanonicalTip {
        height: 6,
        hash: [0u8; 32],
    };
    let mut plan_b = shielded_call_plan(MockWallet::single(ValuePool::Ironwood, 7), other_tip);
    let bytes_b = take_pczt_bytes(&mut plan_b);
    let err = verify_err(&plan_a, reparse(&bytes_b));
    assert!(err.contains("expiry mismatch"), "got: {err}");
}

#[test]
fn pczt_with_different_zalk_payload_rejected() {
    let plan_a = default_plan();
    let funding = ShieldedFunding::new(Box::new(MockWallet::single(ValuePool::Ironwood, 7)), None);
    let req = TxRequest::Call {
        op_return: vec![0x5a, 0x41, 0x4c, 0x4b, 0x00, 0x02, 0xcc, 0xdd],
    };
    let mut plan_b = match funding
        .plan(&req, &FundContext::new(Network::Regtest, tip()))
        .unwrap()
    {
        FundingPlan::Shielded(p) => p,
        _ => unreachable!(),
    };
    let bytes_b = take_pczt_bytes(&mut plan_b);
    let err = verify_err(&plan_a, reparse(&bytes_b));
    assert!(
        err.contains("transparent output 0 script mismatch"),
        "got: {err}"
    );
}

#[test]
fn pczt_funded_from_wrong_pool_rejected() {
    // Plan expects Ironwood funding; the PCZT was funded from Orchard. The
    // spend surfaces in the wrong pool (extra spend / dangling change slot),
    // and the Ironwood side has no actions at all.
    let plan_a = default_plan();
    let mut plan_b = shielded_call_plan(MockWallet::single(ValuePool::Orchard, 7), tip());
    let bytes_b = take_pczt_bytes(&mut plan_b);
    let err = verify_err(&plan_a, reparse(&bytes_b));
    // Any of these manifests the wrong-pool rejection: the Orchard bundle is
    // not the planned change-only bundle (anchor/spends differ), and the
    // Ironwood bundle is empty instead of carrying the planned spend.
    assert!(
        err.contains("extra shielded spend")
            || err.contains("action count mismatch")
            || err.contains("anchor mismatch")
            || err.contains("change"),
        "got: {err}"
    );
}

// ── Freshness (stale shielded authorization) ─────────────────────────────────

struct MockTipSource(CanonicalTip);
impl TipSource for MockTipSource {
    fn canonical_tip(&self) -> Result<CanonicalTip> {
        Ok(self.0)
    }
}

#[test]
fn stale_shielded_prove_sign_extract_rejected() {
    let plan = default_plan();
    let mut plan = FundingPlan::Shielded(plan);
    let stale = MockTipSource(CanonicalTip {
        height: 2,
        hash: [0x77; 32],
    });
    // Freshness is enforced before any proving work begins.
    assert!(plan.prove(&stale).is_err(), "stale prove must fail");
    assert!(plan.sign(&stale).is_err(), "stale sign must fail");
    assert!(plan.extract(&stale).is_err(), "stale extract must fail");
    match plan.extract_verified(&stale) {
        Ok(_) => panic!("stale extract_verified must fail"),
        Err(e) => assert!(e.to_string().contains("StalePlan"), "got: {e}"),
    }
}

/// Regression for the MissingSpendAuthSig live failure: the change action's
/// fabricated zero-valued spend (no dummy_sk) must be covered by the plan's
/// signing list, and every dummy spend must be io-finalizer-signed, so the
/// Signer round-trip leaves EVERY action signed in both pools.
#[test]
fn all_actions_signed_after_signer_round_trip() {
    let mut plan = default_plan();
    let pczt = plan.pczt.take().unwrap();

    let count_sigs = |pczt: pczt::Pczt, label: &str| -> pczt::Pczt {
        let mut o = (0usize, 0usize);
        let mut i = (0usize, 0usize);
        let v = pczt::roles::verifier::Verifier::new(pczt);
        let v = v
            .with_orchard::<String, _>(|b| {
                o = (
                    b.actions().len(),
                    b.actions()
                        .iter()
                        .filter(|a| a.spend().spend_auth_sig().is_some())
                        .count(),
                );
                for (n, a) in b.actions().iter().enumerate() {
                    println!(
                        "  orchard action {n}: sig={} spend_value={:?} out_value={:?} dummy_sk={}",
                        a.spend().spend_auth_sig().is_some(),
                        a.spend().value().map(|v| v.inner()),
                        a.output().value().map(|v| v.inner()),
                        a.spend().dummy_sk().is_some(),
                    );
                }
                Ok(())
            })
            .unwrap();
        let v = v
            .with_ironwood::<String, _>(|b| {
                i = (
                    b.actions().len(),
                    b.actions()
                        .iter()
                        .filter(|a| a.spend().spend_auth_sig().is_some())
                        .count(),
                );
                Ok(())
            })
            .unwrap();
        println!(
            "{label}: orchard {}/{} signed, ironwood {}/{} signed",
            o.1, o.0, i.1, i.0
        );
        v.finish()
    };

    let pczt = count_sigs(pczt, "after plan (io_finalized)");
    // Production sequence minus proving: Signer round-trip.
    let mut signer = pczt::roles::signer::Signer::new(pczt).unwrap();
    for (idx, ask) in &plan.orchard_sign {
        signer.sign_orchard(*idx, ask).unwrap();
    }
    for (idx, ask) in &plan.ironwood_sign {
        signer.sign_ironwood(*idx, ask).unwrap();
    }
    let pczt = signer.finish();
    let mut all_signed = (false, false);
    let v = pczt::roles::verifier::Verifier::new(pczt);
    let v = v
        .with_orchard::<String, _>(|b| {
            all_signed.0 = b
                .actions()
                .iter()
                .all(|a| a.spend().spend_auth_sig().is_some());
            Ok(())
        })
        .unwrap();
    let _ = v
        .with_ironwood::<String, _>(|b| {
            all_signed.1 = b
                .actions()
                .iter()
                .all(|a| a.spend().spend_auth_sig().is_some());
            Ok(())
        })
        .unwrap();
    assert!(all_signed.0, "every orchard action must be signed");
    assert!(all_signed.1, "every ironwood action must be signed");
}
