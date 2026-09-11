//! # zalkanes-wallet
//!
//! Funding layer for Zalkanes PREPARE / DEPLOY / CALL transactions.
//!
//! A [`FundingSource`] turns a funding-pool-agnostic [`TxRequest`] into a
//! [`FundingPlan`] — an inspected, not-yet-authorized transaction. Authorization
//! then proceeds through the explicit stages [`FundingPlan::prove`],
//! [`FundingPlan::sign`], and [`FundingPlan::extract`], so a caller can
//! `--dry-run` inspect everything before generating a single proof or
//! signature.
//!
//! Two sources are provided:
//!
//! - [`TransparentFunding`] — spends P2PKH transparent UTXOs via the existing
//!   `zalkanes-tx` V5/ZIP-244 builder (transparent change).
//! - [`ShieldedFunding`] (feature `shielded`) — spends Orchard/Ironwood notes
//!   and returns shielded change via the lower-level `zcash_primitives`
//!   builder + `pczt` roles.
//!
//! The ZALK message (OP_RETURN payload + P2SH carrier) is byte-identical across
//! pools because it is produced by shared helpers in `zalkanes-tx` from the
//! same [`TxRequest`]. This crate does **not** claim private smart-contract
//! execution: the ZALK message and carrier are always on-chain in cleartext.
//!
//! See `docs/adr/0007-wallet-funding.md` for the design.

#![forbid(unsafe_code)]

#[cfg(feature = "shielded")]
pub mod broadcast;
#[cfg(feature = "shielded")]
pub mod chain_source;
pub mod funding;
#[cfg(feature = "shielded")]
pub mod journal;
pub mod plan;
pub mod policy;
#[cfg(feature = "shielded")]
pub mod shielded;
#[cfg(feature = "shielded")]
pub mod sqlite;
#[cfg(test)]
mod tamper_matrix;
#[cfg(all(test, feature = "shielded"))]
mod tamper_matrix_shielded;
pub mod transparent;

pub use funding::{CanonicalTip, FundContext, FundingPool, FundingSource, FundingUtxo, TxRequest};
pub use plan::{
    commit_plan, FundingPlan, PlanChange, PlanInput, PlanOutput, Stage, TransparentPlan,
};
pub use policy::PrivacyPolicy;
pub use transparent::TransparentFunding;

#[cfg(feature = "shielded")]
pub use broadcast::{
    broadcast_verified, reconcile, BroadcastOutcome, BroadcastTransport, ReconcileReport,
    ReleaseHook, TxStatus, TxStatusSource, ZebraBroadcastClient,
};
#[cfg(feature = "shielded")]
pub use chain_source::{CanonicalChainSource, ZebraCanonicalChainSource};
#[cfg(feature = "shielded")]
pub use journal::{Journal, JournalStage};
#[cfg(feature = "shielded")]
pub use shielded::{ShieldedFunding, ShieldedSelection, ShieldedSpend, ShieldedWallet, SyncStatus};
#[cfg(feature = "shielded")]
pub use sqlite::SqliteShieldedWallet;
