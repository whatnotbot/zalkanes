//! Shielded funding (Orchard/Ironwood) via the lower-level canonical Zcash
//! stack: `zcash_client_sqlite` note selection → `zcash_primitives` transaction
//! `Builder` → custom ZALK transparent outputs → `build_for_pczt` → PCZT
//! `Creator` → `IoFinalizer` → `Prover` → `Signer` → `TransactionExtractor`.
//!
//! No shielded cryptography is implemented here: all proofs/signatures come
//! from `orchard` / `pczt` canonical roles.

#![forbid(unsafe_code)]

use std::convert::Infallible;

use anyhow::{anyhow, bail, Result};
use orchard::{
    builder::BundleMetadata,
    circuit::OrchardCircuitVersion,
    keys::{FullViewingKey, OutgoingViewingKey, SpendAuthorizingKey},
    tree::MerklePath,
    Address, Anchor, Note, ValuePool,
};
use rand_core::OsRng;
use zalkanes_core::consensus_params::ConsensusParams;
use zalkanes_tx::SignedTx;
use zcash_primitives::transaction::{
    builder::{BuildConfig, Builder, BundlePadding},
    fees::zip317::FeeRule,
    TxVersion,
};
use zcash_protocol::{
    consensus::{BlockHeight, BranchId},
    memo::MemoBytes,
    value::Zatoshis,
};
use zcash_transparent::address::TransparentAddress;

use crate::funding::{FundContext, FundingSource, TxRequest};
use crate::plan::Stage;
use crate::policy::PrivacyPolicy;

/// ZIP-317 minimum fee (two grace actions × 5000 zat).
const MINIMUM_FEE: u64 = 10_000;
/// Maximum number of note-selection rounds before we refuse to converge.
const MAX_SELECT_ITERATIONS: usize = 16;

/// One selected shielded note + witness + keys, ready to spend.
#[derive(Clone)]
pub struct ShieldedSpend {
    pub fvk: FullViewingKey,
    pub ask: SpendAuthorizingKey,
    pub note: Note,
    pub merkle_path: MerklePath,
    pub value: u64,
    /// Which protocol pool the note lives in (Orchard or Ironwood).
    pub pool: ValuePool,
}

/// The result of selecting shielded inputs to fund one transaction.
#[derive(Clone)]
pub struct ShieldedSelection {
    pub spends: Vec<ShieldedSpend>,
    pub selected_value: u64,
    /// The change address (an Orchard/Ironwood address owned by the account).
    pub change_address: Address,
    pub change_fvk: FullViewingKey,
    pub change_ovk: Option<OutgoingViewingKey>,
    /// Which pool the change is returned to.
    pub change_pool: ValuePool,
    pub orchard_anchor: Option<Anchor>,
    pub ironwood_anchor: Option<Anchor>,
    /// The wallet-local output references reserved by this selection (used to
    /// release them later).
    pub output_refs: Vec<zcash_client_backend::wallet::OutputRef>,
    /// The lock owner under which `output_refs` were reserved.
    pub lock_owner: zcash_client_backend::data_api::locking::LockOwner,
}

/// The sync state of the shielded wallet relative to our Zebra tip.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SyncStatus {
    /// The canonical chain tip height as reported by our Zebra.
    pub zebra_tip: u32,
    /// The height the wallet has scanned through.
    pub wallet_scan_tip: u32,
    /// Whether shielded spending is safe (`wallet_scan_tip == zebra_tip`).
    pub synced: bool,
    /// The anchor height a shielded spend would use, if any.
    pub anchor_height: Option<u32>,
}

/// Provider of shielded notes + witnesses + change address + durable output
/// locks (reservations).
///
/// The `zcash_client_sqlite`-backed implementation lives behind this trait so
/// the wallet crate does not hard-code the SQLite store; see
/// `crates/zalkanes-wallet/src/sqlite.rs`.
pub trait ShieldedWallet {
    /// Reports the wallet's scan tip against `zebra_tip` and whether shielded
    /// spending is safe.
    fn sync_status(&self, zebra_tip: u32) -> Result<SyncStatus>;

    /// Select notes covering at least `required_zat`, atomically reserving them
    /// under a lock derived from `plan_id`, and return the spends, change
    /// address, anchors, and the reservation handles needed to release them.
    ///
    /// Must fail (rather than return stale witnesses) if the wallet is not
    /// synced to the caller's target height, and must fail (rather than
    /// double-select) if any selected note is already reserved by a different
    /// plan.
    fn select_spends(&self, required_zat: u64, plan_id: &str) -> Result<ShieldedSelection>;

    /// Release the reservations held by `plan_id` (used on cancellation,
    /// proof/signing failure, or known broadcast rejection).
    fn release(&self, plan_id: &str) -> Result<()>;
}

/// A shielded funding source: Orchard/Ironwood notes selected by a
/// [`ShieldedWallet`]. Applies to PREPARE and CALL only; DEPLOY is a
/// transparent carrier-input transaction (see ADR-0007).
pub struct ShieldedFunding {
    wallet: Box<dyn ShieldedWallet>,
    /// Transparent key whose pubkey appears in the carrier redeem script
    /// (`<pubkey> OP_CHECKSIG OP_NOP`), needed for PREPARE so the later DEPLOY
    /// can spend the carriers. Unused for CALL.
    carrier_key: Option<zalkanes_tx::SigningKey>,
}

impl ShieldedFunding {
    pub fn new(
        wallet: Box<dyn ShieldedWallet>,
        carrier_key: Option<zalkanes_tx::SigningKey>,
    ) -> Self {
        Self {
            wallet,
            carrier_key,
        }
    }
}

impl FundingSource for ShieldedFunding {
    fn pool_name(&self) -> &'static str {
        "shielded"
    }

    fn plan(&self, request: &TxRequest, ctx: &FundContext) -> Result<crate::plan::FundingPlan> {
        match request {
            TxRequest::Call { op_return } => self.plan_call(op_return, ctx),
            TxRequest::Prepare { carrier_values } => {
                let carrier_key = self.carrier_key.as_ref().ok_or_else(|| {
                    anyhow!("shielded PREPARE requires a transparent carrier key")
                })?;
                self.plan_prepare(carrier_key, carrier_values, ctx)
            }
            TxRequest::Deploy { .. } => bail!(
                "DEPLOY is a transparent carrier-input transaction; shielded funding \
                 applies to PREPARE and CALL (see docs/adr/0007-wallet-funding.md)"
            ),
        }
    }
}

impl ShieldedFunding {
    fn plan_call(&self, op_return: &[u8], ctx: &FundContext) -> Result<crate::plan::FundingPlan> {
        let params = ConsensusParams::for_network(ctx.network);
        let target_height = BlockHeight::from_u32(ctx.target_height);
        let branch_id = ctx.branch_id();

        let request = TxRequest::Call {
            op_return: op_return.to_vec(),
        };
        let plan_id = crate::plan::new_plan_id();
        let intent_hash = crate::plan::intent_hash_of(
            ctx.network.zebra_name(),
            ctx.target_height,
            "shielded",
            &request,
        );

        // Deterministic, bounded fee/change selection loop: transparent output
        // value is zero for CALL, so `selected = fee + change`.
        let (sel, fee) = select_for(
            &*self.wallet,
            &params,
            target_height,
            branch_id,
            &[],
            op_return,
            0,
            &plan_id,
        )?;
        let change = sel.selected_value - fee;

        let (pczt, orchard_meta, ironwood_meta, tx_version, expiry_height) =
            assemble_and_build(&params, target_height, &sel, &[], op_return, change)?;

        let (orchard_sign, ironwood_sign) = signing_plans(&sel, &orchard_meta, &ironwood_meta)?;

        Ok(crate::plan::FundingPlan::Shielded(Box::new(ShieldedPlan {
            stage: Stage::Planned,
            selected_value: sel.selected_value,
            fee,
            change,
            branch_id,
            target_height: ctx.target_height,
            tx_version,
            expiry_height,
            request,
            pczt: Some(pczt),
            orchard_sign,
            ironwood_sign,
            circuit_version: circuit_version(branch_id)?,
            signed: None,
            plan_id,
            intent_hash,
        })))
    }

    fn plan_prepare(
        &self,
        carrier_key: &zalkanes_tx::SigningKey,
        carrier_values: &[u64],
        ctx: &FundContext,
    ) -> Result<crate::plan::FundingPlan> {
        if carrier_values.is_empty() {
            bail!("PREPARE requires at least one carrier output");
        }
        let params = ConsensusParams::for_network(ctx.network);
        let target_height = BlockHeight::from_u32(ctx.target_height);
        let branch_id = ctx.branch_id();

        let redeem = zalkanes_tx::redeem_script(&carrier_key.compressed_pubkey());
        let carrier_hash = zcash_transparent::util::hash160::hash(&redeem);
        let carrier_addr = TransparentAddress::ScriptHash(carrier_hash);

        let carriers: Vec<(TransparentAddress, u64)> =
            carrier_values.iter().map(|v| (carrier_addr, *v)).collect();
        let carrier_total: u64 = carrier_values.iter().sum();

        let request = TxRequest::Prepare {
            carrier_values: carrier_values.to_vec(),
        };
        let plan_id = crate::plan::new_plan_id();
        let intent_hash = crate::plan::intent_hash_of(
            ctx.network.zebra_name(),
            ctx.target_height,
            "shielded",
            &request,
        );

        let (sel, fee) = select_for(
            &*self.wallet,
            &params,
            target_height,
            branch_id,
            &carriers,
            &[],
            carrier_total,
            &plan_id,
        )?;
        let change = sel
            .selected_value
            .checked_sub(carrier_total)
            .and_then(|v| v.checked_sub(fee))
            .ok_or_else(|| anyhow!("shielded PREPARE: insufficient selected value"))?;

        let (pczt, orchard_meta, ironwood_meta, tx_version, expiry_height) =
            assemble_and_build(&params, target_height, &sel, &carriers, &[], change)?;

        let (orchard_sign, ironwood_sign) = signing_plans(&sel, &orchard_meta, &ironwood_meta)?;

        Ok(crate::plan::FundingPlan::Shielded(Box::new(ShieldedPlan {
            stage: Stage::Planned,
            selected_value: sel.selected_value,
            fee,
            change,
            branch_id,
            target_height: ctx.target_height,
            tx_version,
            expiry_height,
            request,
            pczt: Some(pczt),
            orchard_sign,
            ironwood_sign,
            circuit_version: circuit_version(branch_id)?,
            signed: None,
            plan_id,
            intent_hash,
        })))
    }
}

/// A prepared (unproven, unsigned) shielded plan.
pub struct ShieldedPlan {
    pub stage: Stage,
    pub selected_value: u64,
    pub fee: u64,
    pub change: u64,
    pub branch_id: BranchId,
    pub target_height: u32,
    pub tx_version: &'static str,
    pub expiry_height: u32,
    pub request: TxRequest,
    pub pczt: Option<pczt::Pczt>,
    /// (action index, ask) pairs for the Orchard bundle, in signing order.
    pub orchard_sign: Vec<(usize, SpendAuthorizingKey)>,
    /// (action index, ask) pairs for the Ironwood bundle, in signing order.
    pub ironwood_sign: Vec<(usize, SpendAuthorizingKey)>,
    pub circuit_version: OrchardCircuitVersion,
    pub signed: Option<SignedTx>,
    pub plan_id: String,
    pub intent_hash: String,
}

impl ShieldedPlan {
    pub fn minimum_policy(&self) -> PrivacyPolicy {
        match &self.request {
            // A shielded CALL spends shielded notes and emits only a zero-value
            // OP_RETURN + shielded change: no value/address is revealed.
            TxRequest::Call { .. } => PrivacyPolicy::FullPrivacy,
            // A shielded PREPARE deshields value into transparent carrier
            // outputs, revealing amounts.
            TxRequest::Prepare { .. } => PrivacyPolicy::AllowRevealedAmounts,
            TxRequest::Deploy { .. } => PrivacyPolicy::AllowFullyTransparent,
        }
    }

    pub fn zalk_payload_hex(&self) -> String {
        match self.request.op_return_payload() {
            Some(p) => hex::encode(p),
            None => match &self.request {
                TxRequest::Prepare { carrier_values } => {
                    format!(
                        "carrier outputs x{} ({} zat total)",
                        carrier_values.len(),
                        carrier_values.iter().sum::<u64>()
                    )
                }
                _ => "n/a".to_string(),
            },
        }
    }

    pub fn describe_lines(&self) -> Vec<String> {
        vec![
            format!(
                "Shielded spends:    {} note(s), {} zat",
                self.orchard_sign.len() + self.ironwood_sign.len(),
                self.selected_value
            ),
            format!("Shielded change:    {} zat", self.change),
        ]
    }

    fn expect_stage(&self, expected: Stage) -> Result<()> {
        if self.stage != expected {
            bail!(
                "shielded plan is at stage {:?}, expected {:?}",
                self.stage,
                expected
            );
        }
        Ok(())
    }

    pub(crate) fn prove(&mut self) -> Result<()> {
        self.expect_stage(Stage::Planned)?;
        let pczt = self
            .pczt
            .take()
            .ok_or_else(|| anyhow!("shielded plan has no PCZT"))?;
        let pk = zcash_primitives::transaction::builder::cached_orchard_proving_key(
            self.circuit_version,
        );
        let prover = pczt::roles::prover::Prover::new(pczt);
        let prover = if prover.requires_orchard_proof() {
            prover
                .create_orchard_proof(pk)
                .map_err(|e| anyhow!("orchard prove: {e:?}"))?
        } else {
            prover
        };
        let prover = if prover.requires_ironwood_proof() {
            prover
                .create_ironwood_proof(pk)
                .map_err(|e| anyhow!("ironwood prove: {e:?}"))?
        } else {
            prover
        };
        self.pczt = Some(prover.finish());
        self.stage = Stage::Proven;
        Ok(())
    }

    pub(crate) fn sign(&mut self) -> Result<()> {
        self.expect_stage(Stage::Proven)?;
        let pczt = self
            .pczt
            .take()
            .ok_or_else(|| anyhow!("shielded plan has no PCZT"))?;
        let mut signer =
            pczt::roles::signer::Signer::new(pczt).map_err(|e| anyhow!("signer init: {e:?}"))?;
        for (index, ask) in &self.orchard_sign {
            signer
                .sign_orchard(*index, ask)
                .map_err(|e| anyhow!("orchard sign {index}: {e:?}"))?;
        }
        for (index, ask) in &self.ironwood_sign {
            signer
                .sign_ironwood(*index, ask)
                .map_err(|e| anyhow!("ironwood sign {index}: {e:?}"))?;
        }
        self.pczt = Some(signer.finish());
        self.stage = Stage::Signed;
        Ok(())
    }

    pub(crate) fn extract(&mut self) -> Result<SignedTx> {
        self.expect_stage(Stage::Signed)?;
        let pczt = self
            .pczt
            .take()
            .ok_or_else(|| anyhow!("shielded plan has no PCZT"))?;
        let tx = pczt::roles::tx_extractor::TransactionExtractor::new(pczt)
            .extract()
            .map_err(|e| anyhow!("tx extract: {e:?}"))?;
        let mut bytes = Vec::new();
        tx.write(&mut bytes)
            .map_err(|e| anyhow!("tx serialize: {e:?}"))?;
        let mut txid = [0u8; 32];
        txid.copy_from_slice(tx.txid().as_ref());
        self.signed = Some(SignedTx { bytes, txid });
        self.stage = Stage::Extracted;
        Ok(self.signed.clone().expect("just set"))
    }
}

/// Deterministically select notes covering `required_value + fee`, returning the
/// selection and the (exact) fee. Bounded by [`MAX_SELECT_ITERATIONS`].
#[allow(clippy::too_many_arguments)]
fn select_for(
    wallet: &dyn ShieldedWallet,
    params: &ConsensusParams,
    target_height: BlockHeight,
    branch_id: BranchId,
    carriers: &[(TransparentAddress, u64)],
    op_return: &[u8],
    required_value: u64,
    plan_id: &str,
) -> Result<(ShieldedSelection, u64)> {
    let fee_rule = FeeRule::standard();
    let mut required = required_value
        .checked_add(MINIMUM_FEE)
        .ok_or_else(|| anyhow!("required value overflow"))?;

    for _ in 0..MAX_SELECT_ITERATIONS {
        let sel = wallet.select_spends(required, plan_id)?;
        // Measure the exact fee with one change output present (fee depends only
        // on action counts/sizes, never on values).
        let fee = {
            let builder = assemble_builder(
                params,
                target_height,
                branch_id,
                &sel,
                carriers,
                op_return,
                1,
            )?;
            u64::from(
                builder
                    .get_fee(&fee_rule)
                    .map_err(|e| anyhow!("fee: {e:?}"))?,
            )
        };
        let total_needed = required_value
            .checked_add(fee)
            .ok_or_else(|| anyhow!("required+fee overflow"))?;
        // Require at least 1 zat of change so a change output is always present
        // and the measured fee matches the built fee.
        if sel.selected_value > total_needed {
            return Ok((sel, fee));
        }
        required = total_needed + 1;
    }
    bail!("shielded selection did not converge within {MAX_SELECT_ITERATIONS} rounds")
}

/// Assemble a `zcash_primitives` builder from a selection + ZALK outputs.
#[allow(clippy::too_many_arguments)]
fn assemble_builder(
    params: &ConsensusParams,
    target_height: BlockHeight,
    branch_id: BranchId,
    sel: &ShieldedSelection,
    carriers: &[(TransparentAddress, u64)],
    op_return: &[u8],
    change: u64,
) -> Result<Builder<ConsensusParams, ()>> {
    let mut builder = Builder::new(
        *params,
        target_height,
        BuildConfig::Standard {
            sapling_anchor: None,
            orchard_anchor: sel.orchard_anchor,
            ironwood_anchor: sel.ironwood_anchor,
            orchard_padding: BundlePadding::DEFAULT,
            ironwood_padding: BundlePadding::DEFAULT,
        },
    );
    let _ = branch_id;

    for s in &sel.spends {
        match s.pool {
            ValuePool::Orchard => builder
                .add_orchard_spend::<Infallible>(s.fvk.clone(), s.note, s.merkle_path.clone())
                .map_err(|e| anyhow!("orchard spend: {e:?}"))?,
            ValuePool::Ironwood => builder
                .add_ironwood_spend::<Infallible>(s.fvk.clone(), s.note, s.merkle_path.clone())
                .map_err(|e| anyhow!("ironwood spend: {e:?}"))?,
        }
    }

    if !op_return.is_empty() {
        builder
            .add_transparent_null_data_output::<Infallible>(op_return)
            .map_err(|e| anyhow!("op_return: {e:?}"))?;
    }
    for (addr, value) in carriers {
        builder
            .add_transparent_output(addr, Zatoshis::from_u64(*value)?)
            .map_err(|e| anyhow!("carrier output: {e:?}"))?;
    }
    if change > 0 {
        let value = Zatoshis::from_u64(change)?;
        match sel.change_pool {
            ValuePool::Orchard => builder
                .add_orchard_change_output::<Infallible>(
                    sel.change_fvk.clone(),
                    sel.change_ovk.clone(),
                    sel.change_address,
                    value,
                    MemoBytes::empty(),
                )
                .map_err(|e| anyhow!("orchard change: {e:?}"))?,
            ValuePool::Ironwood => builder
                .add_ironwood_output::<Infallible>(
                    sel.change_ovk.clone(),
                    sel.change_address,
                    value,
                    MemoBytes::empty(),
                )
                .map_err(|e| anyhow!("ironwood change: {e:?}"))?,
        }
    }

    Ok(builder)
}

/// Run `build_for_pczt` → `Creator` → `IoFinalizer`, returning the unproven
/// PCZT plus the metadata needed to map spends to action indices.
#[allow(clippy::type_complexity)]
fn assemble_and_build(
    params: &ConsensusParams,
    target_height: BlockHeight,
    sel: &ShieldedSelection,
    carriers: &[(TransparentAddress, u64)],
    op_return: &[u8],
    change: u64,
) -> Result<(
    pczt::Pczt,
    BundleMetadata,
    BundleMetadata,
    &'static str,
    u32,
)> {
    let branch_id = BranchId::for_height(params, target_height);
    let builder = assemble_builder(
        params,
        target_height,
        branch_id,
        sel,
        carriers,
        op_return,
        change,
    )?;
    let build_result = builder
        .build_for_pczt(OsRng, &FeeRule::standard())
        .map_err(|e| anyhow!("build_for_pczt: {e:?}"))?;

    let expiry_height = u32::from(build_result.pczt_parts.expiry_height);
    let tx_version = match build_result.pczt_parts.version {
        TxVersion::V6 => "v6",
        _ => "v5",
    };

    let pczt = pczt::roles::creator::Creator::build_from_parts(build_result.pczt_parts)
        .ok_or_else(|| anyhow!("PCZT creator rejected transaction parts"))?;
    let pczt = pczt::roles::io_finalizer::IoFinalizer::new(pczt)
        .finalize_io()
        .map_err(|e| anyhow!("io finalize: {e:?}"))?;

    Ok((
        pczt,
        build_result.orchard_meta,
        build_result.ironwood_meta,
        tx_version,
        expiry_height,
    ))
}

/// Map each selected spend to its (possibly randomized) action index, so the
/// Signer signs the right action in each bundle.
#[allow(clippy::type_complexity)]
fn signing_plans(
    sel: &ShieldedSelection,
    orchard_meta: &BundleMetadata,
    ironwood_meta: &BundleMetadata,
) -> Result<(
    Vec<(usize, SpendAuthorizingKey)>,
    Vec<(usize, SpendAuthorizingKey)>,
)> {
    let mut orchard = Vec::new();
    let mut ironwood = Vec::new();
    let mut o = 0usize;
    let mut i = 0usize;
    for s in &sel.spends {
        match s.pool {
            ValuePool::Orchard => {
                let idx = orchard_meta
                    .spend_action_index(o)
                    .ok_or_else(|| anyhow!("orchard spend {o} missing action index"))?;
                orchard.push((idx, s.ask.clone()));
                o += 1;
            }
            ValuePool::Ironwood => {
                let idx = ironwood_meta
                    .spend_action_index(i)
                    .ok_or_else(|| anyhow!("ironwood spend {i} missing action index"))?;
                ironwood.push((idx, s.ask.clone()));
                i += 1;
            }
        }
    }
    Ok((orchard, ironwood))
}

/// The Orchard circuit version for a consensus branch (proving/verification).
fn circuit_version(branch_id: BranchId) -> Result<OrchardCircuitVersion> {
    match branch_id {
        BranchId::Nu6_2 => Ok(OrchardCircuitVersion::FixedPostNu6_2),
        BranchId::Nu6_3 => Ok(OrchardCircuitVersion::PostNu6_3),
        other => bail!("unsupported branch {other:?} for Orchard proving (need Nu6_2 or Nu6_3+)"),
    }
}
