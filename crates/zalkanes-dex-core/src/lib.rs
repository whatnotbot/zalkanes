//! # zalkanes-dex-core — SUBFROST AMM v0 protocol layer
//!
//! Pure, deterministic core of the SUBFROST-style constant-product AMM
//! ported to Zalkanes. This crate contains only:
//!
//! - canonical DEX types ([`types`])
//! - frozen v0 constants ([`v0`])
//! - deterministic integer arithmetic ([`math`], [`wide`])
//! - the stable error model ([`error`])
//! - calldata/event byte encodings ([`encode`], [`events`])
//! - the proposed contract host ABI ([`host`])
//!
//! It is `no_std` + `alloc` so the same code runs unchanged inside
//! wasm32 contracts and in host-side tooling.
//!
//! ## Consensus posture
//!
//! Nothing in this crate touches existing Zalkanes consensus behavior.
//! The [`host::Host`] trait is a *proposed* ABI extension (see
//! `docs/subfrost-amm-v0.md` §"Upstream blockers"); the frozen v0 host
//! ABI of Zalkanes cannot execute these contracts yet.
//!
//! This implementation ports the AMM architecture of SUBFROST/Alkanes to
//! Zalkanes. It does not implement SUBFROST's FROST Bitcoin-custody
//! subsystem.

#![no_std]
#![forbid(unsafe_code)]
#![deny(clippy::float_arithmetic)]

extern crate alloc;

#[cfg(feature = "platform")]
pub mod encode;
pub mod error;
#[cfg(feature = "platform")]
pub mod events;
#[cfg(feature = "platform")]
pub mod host;
pub mod math;
#[cfg(feature = "platform")]
pub mod types;
pub mod v0;
pub mod wide;
