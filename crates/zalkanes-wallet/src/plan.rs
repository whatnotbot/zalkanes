//! The inspectable, staged transaction plan.
//!
//! A [`FundingPlan`] is what [`crate::funding::FundingSource::plan`] returns:
//! the fully-specified but **unauthorized** transaction. Authorization proceeds
//! through the explicit stages [`FundingPlan::prove`], [`FundingPlan::sign`],
//! and [`FundingPlan::extract`], so a caller can `--dry-run` inspect everything
//! before generating a single proof or signature.

#![forbid(unsafe_code)]

use anyhow::{bail, Result};
use rand_core::{OsRng, RngCore};
use sha2::{Digest, Sha256};
use zalkanes_tx::SignedTx;
use zcash_protocol::consensus::BranchId;

use crate::funding::TxRequest;
use crate::policy::PrivacyPolicy;

/// A fresh random, hex-encoded plan id. Unique per plan; the same id is retained
/// across `prove`/`sign`/`extract` so the plan is never silently re-selected.
pub fn new_plan_id() -> String {
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    hex::encode(bytes)
}

/// A SHA-256 hash of the immutable transaction intent: the parts of the request
/// that must not change between `plan()` and `extract()`.
pub fn intent_hash_of(
    network: &str,
    target_height: u32,
    pool: &str,
    request: &TxRequest,
) -> String {
    let mut h = Sha256::new();
    h.update(b"zalkanes-plan-v1\0");
    h.update(network.as_bytes());
    h.update(target_height.to_le_bytes());
    h.update(pool.as_bytes());
    match request {
        TxRequest::Prepare { carrier_values } => {
            h.update(b"prepare");
            for v in carrier_values {
                h.update(v.to_le_bytes());
            }
        }
        TxRequest::Deploy {
            chunks,
            carrier_values,
            op_return,
            ..
        } => {
            h.update(b"deploy");
            h.update(op_return);
            for v in carrier_values {
                h.update(v.to_le_bytes());
            }
            for c in chunks {
                h.update((c.len() as u64).to_le_bytes());
            }
        }
        TxRequest::Call { op_return } => {
            h.update(b"call");
            h.update(op_return);
        }
    }
    hex::encode(h.finalize())
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

    // ── Authorization stages ───────────────────────────────────────────────

    /// Produce any zero-knowledge proofs required by this transaction.
    ///
    /// A no-op for transparent plans. For shielded plans this runs the PCZT
    /// Prover role. Calling out of order is an error.
    pub fn prove(&mut self) -> Result<()> {
        match self {
            FundingPlan::Transparent(p) => p.prove(),
            #[cfg(feature = "shielded")]
            FundingPlan::Shielded(p) => p.prove(),
        }
    }

    /// Apply authorizing signatures (transparent and/or shielded).
    pub fn sign(&mut self) -> Result<()> {
        match self {
            FundingPlan::Transparent(p) => p.sign(),
            #[cfg(feature = "shielded")]
            FundingPlan::Shielded(p) => p.sign(),
        }
    }

    /// Extract the final, network-ready serialized transaction.
    pub fn extract(&mut self) -> Result<SignedTx> {
        match self {
            FundingPlan::Transparent(p) => p.extract(),
            #[cfg(feature = "shielded")]
            FundingPlan::Shielded(p) => p.extract(),
        }
    }
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
}

/// Format zatoshi as a fixed 8-decimal ZEC string.
fn zat_to_zec(zat: u64) -> String {
    let whole = zat / 100_000_000;
    let frac = zat % 100_000_000;
    format!("{whole}.{frac:08}")
}
