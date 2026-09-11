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

pub mod funding;
pub mod plan;
pub mod policy;
#[cfg(feature = "shielded")]
pub mod shielded;
#[cfg(feature = "shielded")]
pub mod sqlite;
pub mod transparent;

pub use funding::{FundContext, FundingPool, FundingSource, FundingUtxo, TxRequest};
pub use plan::{FundingPlan, Stage, TransparentPlan};
pub use policy::PrivacyPolicy;
pub use transparent::TransparentFunding;

#[cfg(feature = "shielded")]
pub use shielded::{ShieldedFunding, ShieldedSelection, ShieldedSpend, ShieldedWallet, SyncStatus};
#[cfg(feature = "shielded")]
pub use sqlite::SqliteShieldedWallet;
