//! # zalkanes-core
//!
//! Consensus constants, shared types, and network configuration.
//! No external I/O. Every other Zalkanes crate depends on this.
//!
//! **Consensus-critical.** Changes to types or constants require an ADR and
//! new test vectors.

#![forbid(unsafe_code)]

pub mod consensus;
pub mod error;
pub mod types;

pub use types::{
    BlockHash, BlockHeight, BlockRef, CodeHash, ContractId, Execution, Network, StateRoot, TxId,
};
