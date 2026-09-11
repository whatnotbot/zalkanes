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

/// One planned shielded spend, as the finalized PCZT must realize it: the
/// exact action index (from the builder's [`BundleMetadata`]), the exact
/// nullifier (derived at plan time via `note.nullifier(&fvk)`), and the exact
/// note value.
#[derive(Clone)]
pub(crate) struct ExpectedSpend {
    pub(crate) pool: ValuePool,
    pub(crate) action_index: usize,
    pub(crate) nullifier: [u8; 32],
    pub(crate) value: u64,
}

/// The planned shielded change output, as the finalized PCZT must realize it.
#[derive(Clone)]
pub(crate) struct ExpectedChange {
    pub(crate) pool: ValuePool,
    pub(crate) action_index: usize,
    pub(crate) value: u64,
    /// Canonical raw receiver bytes of the change address.
    pub(crate) address_bytes: [u8; 43],
    /// Internal-scope incoming viewing key, used to trial-decrypt the change
    /// output ciphertext with the canonical library (no custom note crypto).
    pub(crate) ivk: orchard::keys::IncomingViewingKey,
}

/// Everything the plan expects of the finalized PCZT, recorded at PLAN time
/// from the committed selection — never re-derived from the object under
/// verification.
#[derive(Clone)]
pub(crate) struct ShieldedExpectations {
    pub(crate) spends: Vec<ExpectedSpend>,
    pub(crate) change: ExpectedChange,
    /// Exact action counts per pool in the built transaction (spend/output
    /// padding included), so extra actions cannot be smuggled in.
    pub(crate) actions_orchard: usize,
    pub(crate) actions_ironwood: usize,
    /// The anchor roots handed to the builder, per pool (None = pool absent).
    pub(crate) anchor_orchard: Option<[u8; 32]>,
    pub(crate) anchor_ironwood: Option<[u8; 32]>,
    /// Expected per-pool net value (spends − outputs), zat: the value-balance
    /// each pool's bundle must carry in the final transaction.
    pub(crate) net_orchard: i64,
    pub(crate) net_ironwood: i64,
}

/// A finalized PCZT that has passed exact plan-vs-PCZT verification
/// ([`ShieldedPlan::verify_finalized_pczt`]).
///
/// Type-state boundary: all fields are private and there is no public or
/// unchecked constructor, so an arbitrary finalized PCZT cannot enter the
/// production extractor — only the verification path can produce this type,
/// and production extraction accepts only this type.
pub struct VerifiedPczt {
    pczt: pczt::Pczt,
    /// The verified change output's extracted note commitment (cmx). cmx is
    /// effecting data in the final transaction, so this re-binds the verified
    /// change output across extraction.
    change_cmx: [u8; 32],
}

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
    /// Spend authorizing key matching `change_fvk`: in a bundle that disables
    /// cross-address transfers, the change output is paired with a fabricated
    /// zero-valued spend at the change address which must be signed like any
    /// real spend (upstream `add_change_output` contract).
    pub change_ask: SpendAuthorizingKey,
    pub change_ovk: Option<OutgoingViewingKey>,
    /// Which pool the change is returned to.
    pub change_pool: ValuePool,
    /// The height whose end-of-block treestate the anchors below are roots of.
    pub anchor_height: u32,
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
    pub zebra_tip_height: u32,
    /// The canonical chain tip block hash as reported by our Zebra.
    pub zebra_tip_hash: [u8; 32],
    /// The height the wallet has scanned through.
    pub wallet_scan_height: u32,
    /// The block hash the wallet recorded at its scan tip.
    pub wallet_scan_hash: [u8; 32],
    /// Whether shielded spending is safe (height AND hash both match).
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
    /// Reports the wallet's scan tip (height + hash) against the canonical
    /// `tip`, and whether shielded spending is safe (both identity components
    /// must match).
    fn sync_status(&self, tip: crate::funding::CanonicalTip) -> Result<SyncStatus>;

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
        ctx.validate()?;
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
        let expected = build_expectations(&sel, &orchard_meta, &ironwood_meta, &pczt, change)?;
        let journal_inputs = journal_inputs_for(&sel);

        let transparent_outputs = vec![crate::plan::PlanOutput {
            value: 0,
            script: zalkanes_tx::op_return_script(op_return),
        }];
        let intent_hash = commit_shielded(
            ctx,
            branch_id,
            &sel,
            fee,
            change,
            tx_version,
            expiry_height,
            &transparent_outputs,
            op_return,
            2,
        )?;

        Ok(crate::plan::FundingPlan::Shielded(Box::new(ShieldedPlan {
            stage: Stage::Planned,
            selected_value: sel.selected_value,
            fee,
            change,
            change_destination: sel.change_address.to_raw_address_bytes().to_vec(),
            change_pool: pool_tag(sel.change_pool),
            branch_id,
            target_height: ctx.target_height,
            canonical_tip: ctx.canonical_tip(),
            tx_version,
            expiry_height,
            request,
            transparent_outputs,
            pczt: Some(pczt),
            orchard_sign,
            ironwood_sign,
            expected,
            journal_inputs,
            expected_change_cmx: None,
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
        let expected = build_expectations(&sel, &orchard_meta, &ironwood_meta, &pczt, change)?;
        let journal_inputs = journal_inputs_for(&sel);

        let carrier_script = zalkanes_tx::p2sh_script_pubkey(&redeem);
        let transparent_outputs: Vec<crate::plan::PlanOutput> = carrier_values
            .iter()
            .map(|v| crate::plan::PlanOutput {
                value: *v,
                script: carrier_script.clone(),
            })
            .collect();
        let intent_hash = commit_shielded(
            ctx,
            branch_id,
            &sel,
            fee,
            change,
            tx_version,
            expiry_height,
            &transparent_outputs,
            &[],
            0,
        )?;

        Ok(crate::plan::FundingPlan::Shielded(Box::new(ShieldedPlan {
            stage: Stage::Planned,
            selected_value: sel.selected_value,
            fee,
            change,
            change_destination: sel.change_address.to_raw_address_bytes().to_vec(),
            change_pool: pool_tag(sel.change_pool),
            branch_id,
            target_height: ctx.target_height,
            canonical_tip: ctx.canonical_tip(),
            tx_version,
            expiry_height,
            request,
            transparent_outputs,
            pczt: Some(pczt),
            orchard_sign,
            ironwood_sign,
            expected,
            journal_inputs,
            expected_change_cmx: None,
            circuit_version: circuit_version(branch_id)?,
            signed: None,
            plan_id,
            intent_hash,
        })))
    }
}

/// Canonical journal encoding of a selection's inputs: "pool:txid_hex:index"
/// per input (internal txid byte order), comma-joined, in selection order.
fn journal_inputs_for(sel: &ShieldedSelection) -> String {
    sel.spends
        .iter()
        .zip(&sel.output_refs)
        .map(|(s, r)| {
            format!(
                "{}:{}:{}",
                pool_tag(s.pool),
                hex::encode(r.txid().as_ref()),
                r.output_index()
            )
        })
        .collect::<Vec<_>>()
        .join(",")
}

/// Pool tag byte as committed in the plan intent hash (1 = Orchard,
/// 2 = Ironwood).
fn pool_tag(pool: ValuePool) -> u8 {
    match pool {
        ValuePool::Orchard => 1,
        ValuePool::Ironwood => 2,
    }
}

/// Record everything the finalized PCZT must later match, from the committed
/// selection and the builder metadata (action-index mapping) — never from the
/// object under verification.
fn build_expectations(
    sel: &ShieldedSelection,
    orchard_meta: &BundleMetadata,
    ironwood_meta: &BundleMetadata,
    built_pczt: &pczt::Pczt,
    change: u64,
) -> Result<ShieldedExpectations> {
    let mut spends = Vec::with_capacity(sel.spends.len());
    let (mut o, mut i) = (0usize, 0usize);
    let (mut net_orchard, mut net_ironwood) = (0i64, 0i64);
    for s in &sel.spends {
        let (meta, bucket) = match s.pool {
            ValuePool::Orchard => (orchard_meta, &mut o),
            ValuePool::Ironwood => (ironwood_meta, &mut i),
        };
        let action_index = meta
            .spend_action_index(*bucket)
            .ok_or_else(|| anyhow!("spend {bucket} missing action index ({:?})", s.pool))?;
        *bucket += 1;
        let value_i64 =
            i64::try_from(s.value).map_err(|_| anyhow!("spend value exceeds i64 range"))?;
        match s.pool {
            ValuePool::Orchard => net_orchard = net_orchard.saturating_add(value_i64),
            ValuePool::Ironwood => net_ironwood = net_ironwood.saturating_add(value_i64),
        }
        spends.push(ExpectedSpend {
            pool: s.pool,
            action_index,
            nullifier: s.note.nullifier(&s.fvk).to_bytes(),
            value: s.value,
        });
    }

    let change_meta = match sel.change_pool {
        ValuePool::Orchard => orchard_meta,
        ValuePool::Ironwood => ironwood_meta,
    };
    let change_action_index = change_meta
        .output_action_index(0)
        .ok_or_else(|| anyhow!("change output missing action index"))?;
    let change_i64 = i64::try_from(change).map_err(|_| anyhow!("change exceeds i64 range"))?;
    match sel.change_pool {
        ValuePool::Orchard => net_orchard = net_orchard.saturating_sub(change_i64),
        ValuePool::Ironwood => net_ironwood = net_ironwood.saturating_sub(change_i64),
    }

    Ok(ShieldedExpectations {
        spends,
        change: ExpectedChange {
            pool: sel.change_pool,
            action_index: change_action_index,
            value: change,
            address_bytes: sel.change_address.to_raw_address_bytes(),
            ivk: sel.change_fvk.to_ivk(orchard::keys::Scope::Internal),
        },
        actions_orchard: built_pczt.orchard().actions().len(),
        actions_ironwood: built_pczt.ironwood().actions().len(),
        anchor_orchard: sel.orchard_anchor.map(|a| a.to_bytes()),
        anchor_ironwood: sel.ironwood_anchor.map(|a| a.to_bytes()),
        net_orchard,
        net_ironwood,
    })
}

/// Compute the canonical plan commitment for a shielded plan from its concrete
/// assembled parts (post selection/fee/output assembly).
#[allow(clippy::too_many_arguments)]
fn commit_shielded(
    ctx: &FundContext,
    branch_id: BranchId,
    sel: &ShieldedSelection,
    fee: u64,
    change: u64,
    tx_version: &str,
    expiry_height: u32,
    transparent_outputs: &[crate::plan::PlanOutput],
    zalk_payload: &[u8],
    kind: u8,
) -> Result<String> {
    let inputs: Vec<crate::plan::PlanInput> = sel
        .spends
        .iter()
        .zip(&sel.output_refs)
        .map(|(s, r)| crate::plan::PlanInput {
            pool: match s.pool {
                ValuePool::Orchard => 1,
                ValuePool::Ironwood => 2,
            },
            txid: *r.txid().as_ref(),
            output_index: r.output_index(),
            value: s.value,
        })
        .collect();

    // Commit a pool-tagged anchor for every pool actually spent. An absent
    // anchor for a spent pool is an error (no zero-root substitution).
    let mut anchors = Vec::new();
    let mut has_orchard = false;
    let mut has_ironwood = false;
    for s in &sel.spends {
        match s.pool {
            ValuePool::Orchard => has_orchard = true,
            ValuePool::Ironwood => has_ironwood = true,
        }
    }
    if has_orchard {
        let root = sel
            .orchard_anchor
            .ok_or_else(|| anyhow!("Orchard spend without Orchard anchor"))?
            .to_bytes();
        anchors.push(crate::plan::PlanAnchor {
            pool: 1,
            height: sel.anchor_height,
            root,
        });
    }
    if has_ironwood {
        let root = sel
            .ironwood_anchor
            .ok_or_else(|| anyhow!("Ironwood spend without Ironwood anchor"))?
            .to_bytes();
        anchors.push(crate::plan::PlanAnchor {
            pool: 2,
            height: sel.anchor_height,
            root,
        });
    }

    let tx_ver = if tx_version == "v6" { 6 } else { 5 };
    let change_destination = sel.change_address.to_raw_address_bytes().to_vec();

    Ok(crate::plan::commit_plan(
        ctx.network.id_byte(),
        0, // protocol version
        1, // shielded pool
        ctx.chain_tip.height,
        &ctx.chain_tip.hash,
        ctx.target_height,
        u32::from(branch_id),
        tx_ver,
        expiry_height,
        &inputs,
        &anchors,
        transparent_outputs,
        Some(&crate::plan::PlanChange {
            value: change,
            pool: pool_tag(sel.change_pool),
            destination_bytes: change_destination,
        }),
        fee,
        zalk_payload,
        kind,
    ))
}

/// A prepared (unproven, unsigned) shielded plan.
///
/// All fields are crate-private: outside this crate the plan is driven only
/// through [`crate::plan::FundingPlan`]'s staged API, so the PCZT can never be
/// pulled out and extracted around the [`VerifiedPczt`] boundary.
pub struct ShieldedPlan {
    pub(crate) stage: Stage,
    pub(crate) selected_value: u64,
    pub(crate) fee: u64,
    pub(crate) change: u64,
    /// Canonical serialized change destination, retained for the post-extract
    /// invariant layer.
    pub(crate) change_destination: Vec<u8>,
    /// Pool tag of the change output (1 = Orchard, 2 = Ironwood), as committed
    /// in the plan intent hash.
    pub(crate) change_pool: u8,
    pub(crate) branch_id: BranchId,
    pub(crate) target_height: u32,
    pub(crate) canonical_tip: crate::funding::CanonicalTip,
    pub(crate) tx_version: &'static str,
    pub(crate) expiry_height: u32,
    pub(crate) request: TxRequest,
    /// The exact planned transparent output vector (value + scriptPubKey), in
    /// order, used for post-extract structural verification.
    pub(crate) transparent_outputs: Vec<crate::plan::PlanOutput>,
    pub(crate) pczt: Option<pczt::Pczt>,
    /// (action index, ask) pairs for the Orchard bundle, in signing order.
    pub(crate) orchard_sign: Vec<(usize, SpendAuthorizingKey)>,
    /// (action index, ask) pairs for the Ironwood bundle, in signing order.
    pub(crate) ironwood_sign: Vec<(usize, SpendAuthorizingKey)>,
    /// Everything the finalized PCZT must match, recorded at plan time.
    pub(crate) expected: ShieldedExpectations,
    /// Canonical journal encoding of the selected inputs
    /// ("pool:txid_hex:index", comma-joined), for cross-store recovery.
    pub(crate) journal_inputs: String,
    /// The verified change cmx, stashed by extraction for the post-extract
    /// re-binding check.
    pub(crate) expected_change_cmx: Option<[u8; 32]>,
    pub(crate) circuit_version: OrchardCircuitVersion,
    pub(crate) signed: Option<SignedTx>,
    pub(crate) plan_id: String,
    pub(crate) intent_hash: String,
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
        let change_pool = match self.change_pool {
            2 => "ironwood",
            _ => "orchard",
        };
        vec![
            format!(
                "Shielded spends:    {} note(s), {} zat",
                self.expected.spends.len(),
                self.selected_value
            ),
            format!(
                "Shielded change:    {} zat -> {} pool, destination {}",
                self.change,
                change_pool,
                hex::encode(&self.change_destination)
            ),
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

    /// Structural verification for a shielded-funded transaction. This verifies
    /// the observable transparent portion (no transparent funding input for CALL,
    /// the exact ZALK OP_RETURN output, version, expiry, fee) against the plan.
    /// The shielded bundle's exact spends/change are committed in the plan
    /// intent hash and validated by the canonical PCZT pipeline; they are not
    /// re-parsed here (encrypted representation prevents recovering local-wallet
    /// note identifiers from the final transaction without shielded parsing).
    pub(crate) fn verify_extracted(&self, tx: &SignedTx) -> Result<()> {
        use zcash_primitives::transaction::{Transaction, TxVersion};
        let parsed = Transaction::read(&mut &tx.bytes[..], self.branch_id)
            .map_err(|e| anyhow!("parse extracted tx: {e}"))?;

        let expected_version = if self.tx_version == "v6" {
            TxVersion::V6
        } else {
            TxVersion::V5
        };
        if parsed.version() != expected_version {
            bail!(
                "tx version mismatch: got {:?}, expected {}",
                parsed.version(),
                self.tx_version
            );
        }
        if u32::from(parsed.expiry_height()) != self.expiry_height {
            bail!(
                "tx expiry mismatch: got {}, expected {}",
                u32::from(parsed.expiry_height()),
                self.expiry_height
            );
        }

        let bundle = parsed
            .transparent_bundle()
            .ok_or_else(|| anyhow!("extracted tx has no transparent bundle"))?;

        // A shielded CALL/PREPARE has zero transparent funding inputs.
        if !bundle.vin.is_empty() {
            bail!(
                "shielded tx has {} transparent funding input(s)",
                bundle.vin.len()
            );
        }

        // The complete transparent output vector must match the plan exactly:
        // count, order, value, and scriptPubKey.
        if bundle.vout.len() != self.transparent_outputs.len() {
            bail!(
                "transparent output count mismatch: {} vs {}",
                bundle.vout.len(),
                self.transparent_outputs.len()
            );
        }
        for (i, (vout, planned)) in bundle
            .vout
            .iter()
            .zip(&self.transparent_outputs)
            .enumerate()
        {
            if u64::from(vout.value()) != planned.value {
                bail!("transparent output {i} value mismatch");
            }
            if vout.script_pubkey().0 .0.as_slice() != planned.script.as_slice() {
                bail!("transparent output {i} script mismatch");
            }
        }

        // ── Observable shielded bundle data ─────────────────────────────────
        // Anchors, nullifiers, per-pool value balances, and the verified
        // change cmx are all present in the final serialization; re-bind each.
        self.verify_final_pool(
            parsed.orchard_bundle(),
            ValuePool::Orchard,
            self.expected.actions_orchard,
        )?;
        self.verify_final_pool(
            parsed.ironwood_bundle(),
            ValuePool::Ironwood,
            self.expected.actions_ironwood,
        )?;

        // ── ACTUAL final fee via canonical value accounting ─────────────────
        // `Transaction::fee_paid` sums transparent + all shielded value
        // balances. A shielded plan has zero transparent inputs, so the
        // prevout closure must never be consulted; if it is, fail closed.
        let actual_fee = parsed
            .fee_paid(|_| {
                Err::<Option<zcash_protocol::value::Zatoshis>, anyhow::Error>(anyhow!(
                    "unexpected transparent input during fee accounting"
                ))
            })?
            .ok_or_else(|| anyhow!("fee not computable from final transaction"))?;
        if u64::from(actual_fee) != self.fee {
            bail!(
                "final fee mismatch: extracted {}, planned {}",
                u64::from(actual_fee),
                self.fee
            );
        }

        Ok(())
    }

    /// Re-bind one pool's bundle in the FINAL transaction: exact action count,
    /// exact anchor, exact planned nullifier at each planned action index,
    /// exact per-pool value balance, and the verified change cmx.
    fn verify_final_pool(
        &self,
        bundle: Option<
            &orchard::Bundle<orchard::bundle::Authorized, zcash_protocol::value::ZatBalance>,
        >,
        pool: ValuePool,
        expected_actions: usize,
    ) -> Result<()> {
        let Some(bundle) = bundle else {
            if expected_actions != 0 {
                bail!("final tx missing {pool:?} bundle");
            }
            return Ok(());
        };
        if bundle.actions().len() != expected_actions {
            bail!(
                "final {pool:?} action count mismatch: {} vs {}",
                bundle.actions().len(),
                expected_actions
            );
        }

        let expected_anchor = match pool {
            ValuePool::Orchard => self.expected.anchor_orchard,
            ValuePool::Ironwood => self.expected.anchor_ironwood,
        }
        .ok_or_else(|| anyhow!("final {pool:?} bundle present but no planned anchor"))?;
        if bundle.anchor().to_bytes() != expected_anchor {
            bail!("final {pool:?} anchor mismatch");
        }

        for expected in self.expected.spends.iter().filter(|s| s.pool == pool) {
            let action = bundle
                .actions()
                .iter()
                .nth(expected.action_index)
                .ok_or_else(|| anyhow!("final {pool:?} planned spend action missing"))?;
            if action.nullifier().to_bytes() != expected.nullifier {
                bail!(
                    "final {pool:?} nullifier mismatch at action {}",
                    expected.action_index
                );
            }
        }

        let expected_net = match pool {
            ValuePool::Orchard => self.expected.net_orchard,
            ValuePool::Ironwood => self.expected.net_ironwood,
        };
        if i64::from(*bundle.value_balance()) != expected_net {
            bail!("final {pool:?} value balance mismatch");
        }

        // The change cmx verified at the PCZT boundary must survive into the
        // final effecting data at the same action index.
        if self.expected.change.pool == pool {
            let cmx = self
                .expected_change_cmx
                .ok_or_else(|| anyhow!("change cmx not recorded (extraction bypass?)"))?;
            let action = bundle
                .actions()
                .iter()
                .nth(self.expected.change.action_index)
                .ok_or_else(|| anyhow!("final {pool:?} change action missing"))?;
            if action.cmx().to_bytes() != cmx {
                bail!("final {pool:?} change cmx mismatch");
            }
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

    /// Extract the final transaction, routing through the [`VerifiedPczt`]
    /// boundary: the finalized PCZT is verified field-by-field against the
    /// plan BEFORE the canonical extractor runs. There is no raw-PCZT path.
    pub(crate) fn extract(&mut self) -> Result<SignedTx> {
        self.expect_stage(Stage::Signed)?;
        let pczt = self
            .pczt
            .take()
            .ok_or_else(|| anyhow!("shielded plan has no PCZT"))?;
        let verified = self.verify_finalized_pczt(pczt)?;
        self.extract_from_verified(verified)
    }

    /// Exact plan-vs-finalized-PCZT verification (Item 11 §2-§7). Only this
    /// method can construct a [`VerifiedPczt`]. Every check fails closed: a
    /// field the pinned API cannot expose is an error, never a skipped check.
    pub(crate) fn verify_finalized_pczt(&self, pczt: pczt::Pczt) -> Result<VerifiedPczt> {
        // ── Global identity ─────────────────────────────────────────────────
        if *pczt.global().expiry_height() != self.expiry_height {
            bail!(
                "pczt expiry mismatch: got {}, expected {}",
                pczt.global().expiry_height(),
                self.expiry_height
            );
        }
        if *pczt.global().consensus_branch_id() != u32::from(self.branch_id) {
            bail!("pczt consensus branch mismatch");
        }

        // ── Transparent side: zero funding inputs, exact output vector ──────
        if !pczt.transparent().inputs().is_empty() {
            bail!(
                "shielded pczt has {} transparent input(s)",
                pczt.transparent().inputs().len()
            );
        }
        let t_outs = pczt.transparent().outputs();
        if t_outs.len() != self.transparent_outputs.len() {
            bail!(
                "pczt transparent output count mismatch: {} vs {}",
                t_outs.len(),
                self.transparent_outputs.len()
            );
        }
        for (i, (out, planned)) in t_outs.iter().zip(&self.transparent_outputs).enumerate() {
            if *out.value() != planned.value {
                bail!("pczt transparent output {i} value mismatch");
            }
            if out.script_pubkey().as_slice() != planned.script.as_slice() {
                bail!("pczt transparent output {i} script mismatch");
            }
        }

        // ── Shielded bundles: exact spend/anchor/change binding per pool ────
        let exp = &self.expected;
        let mut change_cmx: Option<[u8; 32]> = None;
        let verifier = pczt::roles::verifier::Verifier::new(pczt);
        let verifier = verifier
            .with_orchard::<String, _>(|bundle| {
                verify_pool_bundle(bundle, ValuePool::Orchard, exp, &mut change_cmx)
                    .map_err(pczt::roles::verifier::OrchardError::Custom)
            })
            .map_err(|e| anyhow!("orchard bundle verification failed: {e:?}"))?;
        let verifier = verifier
            .with_ironwood::<String, _>(|bundle| {
                verify_pool_bundle(bundle, ValuePool::Ironwood, exp, &mut change_cmx)
                    .map_err(pczt::roles::verifier::OrchardError::Custom)
            })
            .map_err(|e| anyhow!("ironwood bundle verification failed: {e:?}"))?;
        let pczt = verifier.finish();

        let change_cmx =
            change_cmx.ok_or_else(|| anyhow!("planned change output not found in pczt"))?;

        Ok(VerifiedPczt { pczt, change_cmx })
    }

    /// Run the canonical extractor over a [`VerifiedPczt`] — the only
    /// production extraction path.
    fn extract_from_verified(&mut self, verified: VerifiedPczt) -> Result<SignedTx> {
        let VerifiedPczt { pczt, change_cmx } = verified;
        let tx = pczt::roles::tx_extractor::TransactionExtractor::new(pczt)
            .extract()
            .map_err(|e| anyhow!("tx extract: {e:?}"))?;
        let mut bytes = Vec::new();
        tx.write(&mut bytes)
            .map_err(|e| anyhow!("tx serialize: {e:?}"))?;
        let mut txid = [0u8; 32];
        txid.copy_from_slice(tx.txid().as_ref());
        self.expected_change_cmx = Some(change_cmx);
        self.signed = Some(SignedTx { bytes, txid });
        self.stage = Stage::Extracted;
        Ok(self.signed.clone().expect("just set"))
    }
}

/// Verify one pool's bundle in the finalized PCZT against the plan
/// expectations. `bundle` is the parsed rich view ([`orchard::pczt::Bundle`])
/// provided by the canonical Verifier role.
fn verify_pool_bundle(
    bundle: &orchard::pczt::Bundle,
    pool: ValuePool,
    exp: &ShieldedExpectations,
    change_cmx: &mut Option<[u8; 32]>,
) -> Result<(), String> {
    let expected_actions = match pool {
        ValuePool::Orchard => exp.actions_orchard,
        ValuePool::Ironwood => exp.actions_ironwood,
    };
    if bundle.actions().len() != expected_actions {
        return Err(format!(
            "{pool:?} action count mismatch: {} vs {}",
            bundle.actions().len(),
            expected_actions
        ));
    }
    if expected_actions == 0 {
        return Ok(());
    }

    // Pool identity of the bundle itself.
    if bundle.bundle_version().value_pool() != pool {
        return Err(format!("{pool:?} bundle carries wrong value pool"));
    }

    // Anchor binding: the bundle anchor must equal the exact root the plan
    // committed for this pool. No zero substitute, no absent-anchor pass.
    let expected_anchor = match pool {
        ValuePool::Orchard => exp.anchor_orchard,
        ValuePool::Ironwood => exp.anchor_ironwood,
    };
    let expected_anchor =
        expected_anchor.ok_or_else(|| format!("{pool:?} bundle present but no planned anchor"))?;
    if bundle.anchor().to_bytes() != expected_anchor {
        return Err(format!("{pool:?} anchor mismatch"));
    }

    let pool_spends: Vec<&ExpectedSpend> = exp.spends.iter().filter(|s| s.pool == pool).collect();

    for (idx, action) in bundle.actions().iter().enumerate() {
        let spend = action.spend();
        match pool_spends.iter().find(|s| s.action_index == idx) {
            Some(expected) => {
                // Planned spend: exact nullifier and exact note value.
                if spend.nullifier().to_bytes() != expected.nullifier {
                    return Err(format!("{pool:?} spend nullifier mismatch at action {idx}"));
                }
                let value = spend
                    .value()
                    .ok_or_else(|| format!("{pool:?} spend value missing at action {idx}"))?;
                if value.inner() != expected.value {
                    return Err(format!("{pool:?} spend value mismatch at action {idx}"));
                }
            }
            None => {
                // Not a planned spend: must be a zero-value dummy. A missing
                // value field is a failure, not a pass.
                let value = spend
                    .value()
                    .ok_or_else(|| format!("{pool:?} dummy spend value missing at action {idx}"))?;
                if value.inner() != 0 {
                    return Err(format!("{pool:?} extra shielded spend at action {idx}"));
                }
            }
        }

        let output = action.output();
        let is_change = exp.change.pool == pool && exp.change.action_index == idx;
        if is_change {
            // Exact change binding: pool, value, destination.
            let recipient = output
                .recipient()
                .ok_or_else(|| format!("{pool:?} change recipient missing at action {idx}"))?;
            if recipient.to_raw_address_bytes() != exp.change.address_bytes {
                return Err(format!("{pool:?} change destination mismatch"));
            }
            let value = output
                .value()
                .ok_or_else(|| format!("{pool:?} change value missing at action {idx}"))?;
            if value.inner() != exp.change.value {
                return Err(format!("{pool:?} change value mismatch"));
            }
            // cmx must commit to exactly the plaintext note fields above
            // (cmx is effecting data in the final transaction).
            output
                .verify_note_commitment(spend)
                .map_err(|e| format!("{pool:?} change note commitment invalid: {e:?}"))?;
            // Independent canonical trial decryption with our own internal
            // viewing key: the ciphertext that reaches the chain must decrypt
            // to the planned change note.
            let prepared = orchard::keys::PreparedIncomingViewingKey::new(&exp.change.ivk);
            let decrypted = match pool {
                ValuePool::Orchard => zcash_note_encryption::try_note_decryption(
                    &orchard::note_encryption::OrchardDomain::for_pczt_action(action),
                    &prepared,
                    action,
                ),
                ValuePool::Ironwood => zcash_note_encryption::try_note_decryption(
                    &orchard::note_encryption::IronwoodDomain::for_pczt_action(action),
                    &prepared,
                    action,
                ),
            };
            let (note, addr, _memo) = decrypted
                .ok_or_else(|| format!("{pool:?} change ciphertext does not decrypt to us"))?;
            if note.value().inner() != exp.change.value
                || addr.to_raw_address_bytes() != exp.change.address_bytes
            {
                return Err(format!("{pool:?} decrypted change note mismatch"));
            }
            *change_cmx = Some(output.cmx().to_bytes());
        } else {
            // Not the planned change: any value-bearing output is unplanned.
            let value = output
                .value()
                .ok_or_else(|| format!("{pool:?} output value missing at action {idx}"))?;
            if value.inner() != 0 {
                return Err(format!(
                    "{pool:?} unplanned value-bearing output at action {idx}"
                ));
            }
        }
    }

    // Pool net value: the bundle's value sum must equal (spends − outputs)
    // exactly as planned. Sign convention: positive = value leaving the pool.
    let expected_net: i128 = i128::from(match pool {
        ValuePool::Orchard => exp.net_orchard,
        ValuePool::Ironwood => exp.net_ironwood,
    });
    let (magnitude, sign) = bundle.value_sum().magnitude_sign();
    let actual_net: i128 = match sign {
        orchard::value::Sign::Positive => i128::from(magnitude),
        orchard::value::Sign::Negative => -i128::from(magnitude),
    };
    if actual_net != expected_net {
        return Err(format!(
            "{pool:?} value balance mismatch: {actual_net} vs {expected_net}"
        ));
    }

    Ok(())
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
/// Signer signs the right action in each bundle. The change output's action is
/// included: its fabricated zero-valued spend (at the change address) must be
/// signed with the change account's spend authorizing key, exactly like a real
/// spend (upstream `add_change_output` contract).
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

    // Only an ORCHARD change output pairs with a fabricated wallet-controlled
    // spend that we must sign (`add_change_output` in a cross-address-disabled
    // bundle). An Ironwood change output is a plain output: its action's
    // spend-half is either a real spend (signed above) or an io-finalizer-
    // signed dummy — signing it with our key would be wrong.
    if sel.change_pool == ValuePool::Orchard {
        let change_idx = orchard_meta
            .output_action_index(0)
            .ok_or_else(|| anyhow!("change output missing action index"))?;
        orchard.push((change_idx, sel.change_ask.clone()));
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
