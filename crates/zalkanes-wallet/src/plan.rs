//! The inspectable, staged transaction plan.
//!
//! A [`FundingPlan`] is what [`crate::funding::FundingSource::plan`] returns:
//! the fully-specified but **unauthorized** transaction. Authorization proceeds
//! through the explicit stages [`FundingPlan::prove`], [`FundingPlan::sign`],
//! and [`FundingPlan::extract`], so a caller can `--dry-run` inspect everything
//! before generating a single proof or signature.

#![forbid(unsafe_code)]

use anyhow::{anyhow, bail, Result};
use rand_core::{OsRng, RngCore};
use sha2::{Digest, Sha256};
use zalkanes_tx::SignedTx;
use zcash_primitives::transaction::{Transaction, TxVersion};
use zcash_protocol::consensus::BranchId;

use crate::funding::{CanonicalTip, TxRequest};
use crate::policy::PrivacyPolicy;

/// A fresh random, hex-encoded plan id. Unique per plan; the same id is retained
/// across `prove`/`sign`/`extract` so the plan is never silently re-selected.
pub fn new_plan_id() -> String {
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    hex::encode(bytes)
}

/// An exact selected input, for the plan commitment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlanInput {
    /// 0 = transparent, 1 = Orchard, 2 = Ironwood.
    pub pool: u8,
    /// The input's stable identity (outpoint txid / note-creating txid).
    pub txid: [u8; 32],
    pub output_index: u32,
    pub value: u64,
}

/// An exact transparent output, for the plan commitment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlanOutput {
    pub value: u64,
    pub script: Vec<u8>,
}

/// A pool-tagged shielded anchor, for the plan commitment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlanAnchor {
    /// 1 = Orchard, 2 = Ironwood.
    pub pool: u8,
    pub height: u32,
    pub root: [u8; 32],
}

/// The exact shielded change, for the plan commitment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlanChange {
    pub value: u64,
    pub pool: u8,
    /// Canonical serialized destination material (the raw receiver address
    /// bytes), so two PCZTs sending the same amount to different shielded
    /// addresses cannot share a plan hash.
    pub destination_bytes: Vec<u8>,
}

/// The canonical, domain-separated binary plan commitment (a SHA-256 over a
/// length-prefixed encoding of the *concrete* assembled plan, not the logical
/// request). Integer widths and byte order are fixed; every variable-length
/// field is length-prefixed.
#[allow(clippy::too_many_arguments)]
pub fn commit_plan(
    network_id: u8,
    protocol_version: u8,
    pool: u8,
    chain_tip_height: u32,
    chain_tip_hash: &[u8; 32],
    target_height: u32,
    branch_id: u32,
    tx_version: u8,
    expiry_height: u32,
    inputs: &[PlanInput],
    anchors: &[PlanAnchor],
    transparent_outputs: &[PlanOutput],
    shielded_change: Option<&PlanChange>,
    fee: u64,
    zalk_payload: &[u8],
    kind: u8,
) -> String {
    let mut buf = Vec::with_capacity(256);
    buf.extend_from_slice(b"ZALKANES_FUNDING_PLAN_V1\x00");
    buf.push(network_id);
    buf.push(protocol_version);
    buf.push(pool);
    buf.extend_from_slice(&chain_tip_height.to_le_bytes());
    buf.extend_from_slice(chain_tip_hash);
    buf.extend_from_slice(&target_height.to_le_bytes());
    buf.extend_from_slice(&branch_id.to_le_bytes());
    buf.push(tx_version);
    buf.extend_from_slice(&expiry_height.to_le_bytes());

    buf.extend_from_slice(&(inputs.len() as u16).to_le_bytes());
    for i in inputs {
        buf.push(i.pool);
        buf.extend_from_slice(&i.txid);
        buf.extend_from_slice(&i.output_index.to_le_bytes());
        buf.extend_from_slice(&i.value.to_le_bytes());
    }

    buf.extend_from_slice(&(anchors.len() as u16).to_le_bytes());
    for a in anchors {
        buf.push(a.pool);
        buf.extend_from_slice(&a.height.to_le_bytes());
        buf.extend_from_slice(&a.root);
    }

    buf.extend_from_slice(&(transparent_outputs.len() as u16).to_le_bytes());
    for o in transparent_outputs {
        buf.extend_from_slice(&o.value.to_le_bytes());
        buf.extend_from_slice(&(o.script.len() as u16).to_le_bytes());
        buf.extend_from_slice(&o.script);
    }

    match shielded_change {
        Some(c) => {
            buf.push(1);
            buf.extend_from_slice(&c.value.to_le_bytes());
            buf.push(c.pool);
            buf.extend_from_slice(&(c.destination_bytes.len() as u16).to_le_bytes());
            buf.extend_from_slice(&c.destination_bytes);
        }
        None => buf.push(0),
    }

    buf.extend_from_slice(&fee.to_le_bytes());

    buf.extend_from_slice(&(zalk_payload.len() as u16).to_le_bytes());
    buf.extend_from_slice(zalk_payload);

    buf.push(kind);

    hex::encode(Sha256::digest(&buf))
}

/// Authorization stage of a [`FundingPlan`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    /// Inputs/outputs/fee/change are fully specified; nothing is proven or
    /// signed yet.
    Planned,
    /// Zero-knowledge proofs have been produced (shielded only; a no-op for
    /// transparent plans).
    Proven,
    /// Transparent (and shielded, if any) authorizing signatures applied.
    Signed,
    /// The final transaction has been extracted and is ready to broadcast.
    Extracted,
}

/// A funding-pool-agnostic plan for one Zalkanes transaction.
///
/// Variants are boxed to keep the enum small (both plans carry substantial
/// state; the plan is returned once and authorized in place).
pub enum FundingPlan {
    Transparent(Box<TransparentPlan>),
    #[cfg(feature = "shielded")]
    Shielded(Box<crate::shielded::ShieldedPlan>),
}

impl FundingPlan {
    pub fn pool_name(&self) -> &'static str {
        match self {
            FundingPlan::Transparent(_) => "transparent",
            #[cfg(feature = "shielded")]
            FundingPlan::Shielded(_) => "shielded",
        }
    }

    pub fn stage(&self) -> Stage {
        match self {
            FundingPlan::Transparent(p) => p.stage,
            #[cfg(feature = "shielded")]
            FundingPlan::Shielded(p) => p.stage,
        }
    }

    /// Sum of the values selected to fund this transaction (transparent UTXO
    /// values or shielded note values).
    pub fn selected_value(&self) -> u64 {
        match self {
            FundingPlan::Transparent(p) => p.selected_value(),
            #[cfg(feature = "shielded")]
            FundingPlan::Shielded(p) => p.selected_value,
        }
    }

    /// The ZIP-317 fee this transaction will pay.
    pub fn fee(&self) -> u64 {
        match self {
            FundingPlan::Transparent(p) => p.prepared.fee,
            #[cfg(feature = "shielded")]
            FundingPlan::Shielded(p) => p.fee,
        }
    }

    /// The change returned to the funding pool (transparent or shielded).
    pub fn change(&self) -> u64 {
        match self {
            FundingPlan::Transparent(p) => p.prepared.change,
            #[cfg(feature = "shielded")]
            FundingPlan::Shielded(p) => p.change,
        }
    }

    /// The minimum Zallet privacy policy this specific transaction requires,
    /// derived from what it actually reveals (value/address), **not** a global
    /// constant.
    pub fn minimum_policy(&self) -> PrivacyPolicy {
        match self {
            // Transparent inputs and outputs reveal the sender address and the
            // amounts.
            FundingPlan::Transparent(_) => PrivacyPolicy::AllowFullyTransparent,
            #[cfg(feature = "shielded")]
            FundingPlan::Shielded(p) => p.minimum_policy(),
        }
    }

    /// The consensus branch id used for signing.
    pub fn branch_id(&self) -> BranchId {
        match self {
            FundingPlan::Transparent(p) => p.branch_id,
            #[cfg(feature = "shielded")]
            FundingPlan::Shielded(p) => p.branch_id,
        }
    }

    /// Target height the transaction is built for.
    pub fn target_height(&self) -> u32 {
        match self {
            FundingPlan::Transparent(p) => p.target_height,
            #[cfg(feature = "shielded")]
            FundingPlan::Shielded(p) => p.target_height,
        }
    }

    /// Transaction version string ("v5" or "v6").
    pub fn tx_version(&self) -> &'static str {
        match self {
            FundingPlan::Transparent(_) => "v5",
            #[cfg(feature = "shielded")]
            FundingPlan::Shielded(p) => p.tx_version,
        }
    }

    /// The expiry height (`0` disables expiry, as in transparent plans).
    pub fn expiry_height(&self) -> u32 {
        match self {
            FundingPlan::Transparent(_) => 0,
            #[cfg(feature = "shielded")]
            FundingPlan::Shielded(p) => p.expiry_height,
        }
    }

    /// The ZALK payload that will be committed on chain (OP_RETURN payload, or a
    /// description of the carrier structure for PREPARE). Hex-encoded.
    pub fn zalk_payload_hex(&self) -> String {
        match self {
            FundingPlan::Transparent(p) => p.zalk_payload_hex(),
            #[cfg(feature = "shielded")]
            FundingPlan::Shielded(p) => p.zalk_payload_hex(),
        }
    }

    /// A multi-line human-readable description for `--dry-run`.
    pub fn describe(&self) -> String {
        let mut lines = Vec::new();
        lines.push(format!("Funding pool:       {}", self.pool_name()));
        lines.push(format!("Stage:              {:?}", self.stage()));
        lines.push(format!(
            "Selected value:     {} zat ({} ZEC)",
            self.selected_value(),
            zat_to_zec(self.selected_value())
        ));
        lines.push(format!(
            "Fee (ZIP-317):      {} zat ({} ZEC)",
            self.fee(),
            zat_to_zec(self.fee())
        ));
        lines.push(format!(
            "Change ({}): {} zat ({} ZEC)",
            self.pool_name(),
            self.change(),
            zat_to_zec(self.change())
        ));
        lines.push(format!("Target height:      {}", self.target_height()));
        lines.push(format!("Transaction version:{}", self.tx_version()));
        lines.push(format!("Expiry height:      {}", self.expiry_height()));
        lines.push(format!("ZALK payload:       {}", self.zalk_payload_hex()));
        lines.push(format!(
            "Min privacy policy: {} ({})",
            self.minimum_policy(),
            self.minimum_policy().describe()
        ));
        lines.push(disclosure_line());
        // Pool-specific details.
        match self {
            FundingPlan::Transparent(p) => lines.extend(p.describe_lines()),
            #[cfg(feature = "shielded")]
            FundingPlan::Shielded(p) => lines.extend(p.describe_lines()),
        }
        lines.join("\n")
    }

    /// The unique, random plan id (stable across `prove`/`sign`/`extract`).
    pub fn plan_id(&self) -> &str {
        match self {
            FundingPlan::Transparent(p) => &p.plan_id,
            #[cfg(feature = "shielded")]
            FundingPlan::Shielded(p) => &p.plan_id,
        }
    }

    /// The SHA-256 hash of the immutable transaction intent.
    pub fn intent_hash(&self) -> &str {
        match self {
            FundingPlan::Transparent(p) => &p.intent_hash,
            #[cfg(feature = "shielded")]
            FundingPlan::Shielded(p) => &p.intent_hash,
        }
    }

    /// The canonical chain identity this plan is pinned to.
    pub fn canonical_tip(&self) -> CanonicalTip {
        match self {
            FundingPlan::Transparent(p) => p.canonical_tip,
            #[cfg(feature = "shielded")]
            FundingPlan::Shielded(p) => p.canonical_tip,
        }
    }

    /// Reject the plan if the canonical chain identity has changed. Call before
    /// prove/sign/extract/broadcast; a change means the plan must be re-made.
    pub fn check_freshness(&self, current: CanonicalTip) -> Result<()> {
        let planned = self.canonical_tip();
        if planned != current {
            bail!(
                "StalePlan: chain state changed (planned {}:{}, current {}:{}); re-plan required",
                planned.height,
                hex::encode(planned.hash),
                current.height,
                hex::encode(current.hash),
            );
        }
        Ok(())
    }

    // ── Authorization stages ───────────────────────────────────────────────
    //
    // Every stage takes the current canonical tip and enforces freshness
    // internally, so a broadcast-capable caller cannot bypass the gate: a plan
    // made against a different chain identity is rejected before any proof,
    // signature, or extraction work happens.

    /// Produce any zero-knowledge proofs required by this transaction.
    ///
    /// A no-op for transparent plans. For shielded plans this runs the PCZT
    /// Prover role. Rejects the plan if `current_tip` no longer matches the
    /// chain identity the plan was pinned to.
    pub fn prove(&mut self, current_tip: CanonicalTip) -> Result<()> {
        self.check_freshness(current_tip)?;
        match self {
            FundingPlan::Transparent(p) => p.prove(),
            #[cfg(feature = "shielded")]
            FundingPlan::Shielded(p) => p.prove(),
        }
    }

    /// Apply authorizing signatures (transparent and/or shielded), after
    /// re-verifying the canonical tip.
    pub fn sign(&mut self, current_tip: CanonicalTip) -> Result<()> {
        self.check_freshness(current_tip)?;
        match self {
            FundingPlan::Transparent(p) => p.sign(),
            #[cfg(feature = "shielded")]
            FundingPlan::Shielded(p) => p.sign(),
        }
    }

    /// Extract the final, network-ready serialized transaction, after
    /// re-verifying the canonical tip.
    pub fn extract(&mut self, current_tip: CanonicalTip) -> Result<SignedTx> {
        self.check_freshness(current_tip)?;
        match self {
            FundingPlan::Transparent(p) => p.extract(),
            #[cfg(feature = "shielded")]
            FundingPlan::Shielded(p) => p.extract(),
        }
    }

    /// Extract and structurally verify the final transaction against this plan.
    /// Returns a [`VerifiedTransaction`], the only type broadcast APIs accept.
    pub fn extract_verified(&mut self, current_tip: CanonicalTip) -> Result<VerifiedTransaction> {
        let tx = self.extract(current_tip)?;
        self.verify_extracted(&tx)?;
        Ok(VerifiedTransaction {
            signed: tx,
            plan_id: self.plan_id().to_string(),
            intent_hash: self.intent_hash().to_string(),
        })
    }

    /// Structurally compare an extracted transaction against this plan. Any
    /// mismatch is fatal; the transaction must never be repaired.
    pub(crate) fn verify_extracted(&self, tx: &SignedTx) -> Result<()> {
        match self {
            FundingPlan::Transparent(p) => p.verify_extracted(tx),
            #[cfg(feature = "shielded")]
            FundingPlan::Shielded(p) => p.verify_extracted(tx),
        }
    }
}

/// A transaction that has been structurally verified against its [`FundingPlan`].
/// Broadcast-capable APIs accept only this type — never a raw transaction.
pub struct VerifiedTransaction {
    pub signed: SignedTx,
    pub plan_id: String,
    pub intent_hash: String,
}

/// The separate Zalkanes disclosure, distinct from Zallet's privacy-policy
/// vocabulary: contract metadata is always public on chain.
fn disclosure_line() -> String {
    // opcode/calldata/code are public regardless of funding pool.
    "Zalkanes disclosure: contract id / opcode / calldata / code are public".to_string()
}

/// A prepared (unsigned) transparent plan, backing [`FundingPlan::Transparent`].
pub struct TransparentPlan {
    /// The unsigned inputs/outputs/fee/change from `zalkanes-tx`.
    pub prepared: zalkanes_tx::PreparedTx,
    pub request: TxRequest,
    pub branch_id: BranchId,
    pub target_height: u32,
    /// The canonical chain identity this plan is pinned to.
    pub canonical_tip: CanonicalTip,
    pub stage: Stage,
    pub signed: Option<SignedTx>,
    pub plan_id: String,
    pub intent_hash: String,
}

impl TransparentPlan {
    fn selected_value(&self) -> u64 {
        self.prepared.selected_value
    }

    fn zalk_payload_hex(&self) -> String {
        match self.request.op_return_payload() {
            Some(p) => hex::encode(p),
            None => format!("carrier outputs x{}", self.prepared.outputs.len()),
        }
    }

    fn describe_lines(&self) -> Vec<String> {
        let mut lines = Vec::new();
        for (i, out) in self.prepared.outputs.iter().enumerate() {
            let kind = if out.script_pubkey.first() == Some(&0x6a) {
                "OP_RETURN (ZALK)"
            } else {
                "transparent"
            };
            lines.push(format!("  output[{i}]: {kind}, {} zat", out.value));
        }
        lines
    }

    fn prove(&mut self) -> Result<()> {
        self.expect_stage(Stage::Planned)?;
        // Transparent transactions have no zero-knowledge proofs.
        self.stage = Stage::Proven;
        Ok(())
    }

    fn sign(&mut self) -> Result<()> {
        self.expect_stage(Stage::Proven)?;
        let tx = zalkanes_tx::build_transparent_tx(
            &self.prepared.inputs,
            &self.prepared.outputs,
            0,
            self.branch_id,
        )?;
        self.signed = Some(tx);
        self.stage = Stage::Signed;
        Ok(())
    }

    fn extract(&mut self) -> Result<SignedTx> {
        self.expect_stage(Stage::Signed)?;
        let tx = self
            .signed
            .take()
            .ok_or_else(|| anyhow::anyhow!("transparent plan has no signed transaction"))?;
        self.stage = Stage::Extracted;
        Ok(tx)
    }

    fn expect_stage(&self, expected: Stage) -> Result<()> {
        if self.stage != expected {
            bail!(
                "transparent plan is at stage {:?}, expected {:?}",
                self.stage,
                expected
            );
        }
        Ok(())
    }

    /// Structural verification for a transparent-funded transaction: the
    /// extracted transaction's transparent inputs, outputs, fee, version, and
    /// expiry must exactly match the plan.
    pub(crate) fn verify_extracted(&self, tx: &SignedTx) -> Result<()> {
        let parsed = Transaction::read(&mut &tx.bytes[..], self.branch_id)
            .map_err(|e| anyhow!("parse extracted tx: {e}"))?;

        if parsed.version() != TxVersion::V5 {
            bail!(
                "tx version mismatch: got {:?}, expected V5",
                parsed.version()
            );
        }
        if u32::from(parsed.expiry_height()) != 0 {
            bail!(
                "tx expiry mismatch: got {}, expected 0",
                u32::from(parsed.expiry_height())
            );
        }

        let bundle = parsed
            .transparent_bundle()
            .ok_or_else(|| anyhow!("extracted tx has no transparent bundle"))?;

        // Exact transparent inputs (prevouts), in order.
        if bundle.vin.len() != self.prepared.inputs.len() {
            bail!(
                "input count mismatch: {} vs {}",
                bundle.vin.len(),
                self.prepared.inputs.len()
            );
        }
        for (i, (vin, plan_in)) in bundle.vin.iter().zip(&self.prepared.inputs).enumerate() {
            if vin.prevout().hash() != plan_in.outpoint.hash()
                || vin.prevout().n() != plan_in.outpoint.n()
            {
                bail!("input {i} prevout mismatch");
            }
        }

        // Exact transparent outputs (value + scriptPubKey), in order.
        if bundle.vout.len() != self.prepared.outputs.len() {
            bail!(
                "output count mismatch: {} vs {}",
                bundle.vout.len(),
                self.prepared.outputs.len()
            );
        }
        for (i, (vout, plan_out)) in bundle.vout.iter().zip(&self.prepared.outputs).enumerate() {
            if u64::from(vout.value()) != plan_out.value {
                bail!("output {i} value mismatch");
            }
            if vout.script_pubkey().0 .0.as_slice() != plan_out.script_pubkey.as_slice() {
                bail!("output {i} script mismatch");
            }
        }

        // Fee via canonical value accounting (inputs - outputs).
        let in_sum: u64 = self.prepared.inputs.iter().map(|i| i.value).sum();
        let out_sum: u64 = self.prepared.outputs.iter().map(|o| o.value).sum();
        let actual_fee = in_sum
            .checked_sub(out_sum)
            .ok_or_else(|| anyhow!("negative fee"))?;
        if actual_fee != self.prepared.fee {
            bail!(
                "fee mismatch: extracted {actual_fee}, planned {}",
                self.prepared.fee
            );
        }

        Ok(())
    }
}

/// Format zatoshi as a fixed 8-decimal ZEC string.
fn zat_to_zec(zat: u64) -> String {
    let whole = zat / 100_000_000;
    let frac = zat % 100_000_000;
    format!("{whole}.{frac:08}")
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Spec {
        network_id: u8,
        protocol_version: u8,
        pool: u8,
        chain_tip_height: u32,
        chain_tip_hash: [u8; 32],
        target_height: u32,
        branch_id: u32,
        tx_version: u8,
        expiry_height: u32,
        inputs: Vec<PlanInput>,
        anchors: Vec<PlanAnchor>,
        outputs: Vec<PlanOutput>,
        change: Option<PlanChange>,
        fee: u64,
        zalk: Vec<u8>,
        kind: u8,
    }

    impl Spec {
        fn base() -> Self {
            Spec {
                network_id: 2,
                protocol_version: 0,
                pool: 1,
                chain_tip_height: 100,
                chain_tip_hash: [1u8; 32],
                target_height: 101,
                branch_id: 0x1234,
                tx_version: 6,
                expiry_height: 120,
                inputs: vec![PlanInput {
                    pool: 1,
                    txid: [2u8; 32],
                    output_index: 0,
                    value: 50_000,
                }],
                anchors: vec![PlanAnchor {
                    pool: 1,
                    height: 100,
                    root: [3u8; 32],
                }],
                outputs: vec![PlanOutput {
                    value: 0,
                    script: vec![0x6a, 0x02, 0x02, 0x00],
                }],
                change: Some(PlanChange {
                    value: 40_000,
                    pool: 1,
                    destination_bytes: vec![9u8; 43],
                }),
                fee: 10_000,
                zalk: vec![0x02, 0x00],
                kind: 2,
            }
        }

        fn commit(&self) -> String {
            commit_plan(
                self.network_id,
                self.protocol_version,
                self.pool,
                self.chain_tip_height,
                &self.chain_tip_hash,
                self.target_height,
                self.branch_id,
                self.tx_version,
                self.expiry_height,
                &self.inputs,
                &self.anchors,
                &self.outputs,
                self.change.as_ref(),
                self.fee,
                &self.zalk,
                self.kind,
            )
        }
    }

    #[test]
    fn identical_serialization_produces_identical_hash() {
        assert_eq!(Spec::base().commit(), Spec::base().commit());
    }

    #[test]
    fn any_field_mutation_changes_hash() {
        let b = Spec::base().commit();

        let mut s = Spec::base();
        s.network_id = 3;
        assert_ne!(b, s.commit(), "network id");

        let mut s = Spec::base();
        s.inputs[0].txid = [9u8; 32];
        assert_ne!(b, s.commit(), "note identity");

        let mut s = Spec::base();
        s.inputs[0].value = 50_001;
        assert_ne!(b, s.commit(), "input value");

        let mut s = Spec::base();
        s.fee = 10_001;
        assert_ne!(b, s.commit(), "fee");

        let mut s = Spec::base();
        s.change.as_mut().unwrap().value = 40_001;
        assert_ne!(b, s.commit(), "change value");

        let mut s = Spec::base();
        s.change.as_mut().unwrap().destination_bytes = vec![7u8; 43];
        assert_ne!(b, s.commit(), "change destination");

        let mut s = Spec::base();
        s.zalk[1] = 0x01;
        assert_ne!(b, s.commit(), "zalk byte");

        let mut s = Spec::base();
        s.outputs[0].value = 1;
        assert_ne!(b, s.commit(), "carrier/output value");

        let mut s = Spec::base();
        s.chain_tip_hash = [7u8; 32];
        assert_ne!(b, s.commit(), "chain tip hash");

        let mut s = Spec::base();
        s.anchors[0].root = [8u8; 32];
        assert_ne!(b, s.commit(), "orchard anchor root");

        let mut s = Spec::base();
        s.anchors.push(PlanAnchor {
            pool: 2,
            height: 100,
            root: [4u8; 32],
        });
        assert_ne!(b, s.commit(), "ironwood anchor added");

        let mut s = Spec::base();
        s.anchors[0].pool = 2;
        assert_ne!(b, s.commit(), "anchor pool tag");

        let mut s = Spec::base();
        s.expiry_height = 121;
        assert_ne!(b, s.commit(), "expiry height");

        let mut s = Spec::base();
        s.tx_version = 5;
        assert_ne!(b, s.commit(), "tx version");

        let mut s = Spec::base();
        s.target_height = 102;
        assert_ne!(b, s.commit(), "target height");
    }
}
