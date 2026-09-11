//! # zalkanes-wallet
//!
//! Funding layer for Zalkanes PREPARE / DEPLOY / CALL transactions.
//!
//! A [`FundingSource`] turns a funding-pool-agnostic [`TxRequest`] into a
//! signed, serialized transaction. Two sources are provided:
//!
//! - [`TransparentFunding`] — spends P2PKH transparent UTXOs via the existing
//!   `zalkanes-tx` V5/ZIP-244 builder (transparent change).
//! - `ShieldedFunding` — spends Orchard/Ironwood notes and returns shielded
//!   change via the lower-level `zcash_primitives` builder + `pczt` roles
//!   (gated behind the `shielded` cargo feature).
//!
//! The ZALK message (OP_RETURN payload + P2SH carrier) is byte-identical across
//! pools because it is produced by shared helpers in `zalkanes-tx` from the
//! same [`TxRequest`]. This crate does **not** claim private smart-contract
//! execution: the ZALK message and carrier are always on-chain in cleartext.
//!
//! See `docs/adr/0007-wallet-funding.md` for the design.

#![forbid(unsafe_code)]

pub mod funding;
pub mod policy;
pub mod transparent;

pub use funding::{FundContext, FundingPool, FundingSource, FundingUtxo, TxRequest};
pub use policy::PrivacyPolicy;
pub use transparent::TransparentFunding;

/// The [`FundingSource`] behind a built transaction, used for logging.
pub const TRANSPARENT_POOL: &str = "transparent";
/// The shielded (Orchard/Ironwood) funding pool name.
pub const SHIELDED_POOL: &str = "shielded";
