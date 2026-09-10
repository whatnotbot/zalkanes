//! # zalkanes-core
//!
//! Consensus constants, shared types, and network configuration.
//! No external I/O. Every other Zalkanes crate depends on this.
//!
//! **Consensus-critical.** Changes to types or constants require an ADR and
//! new test vectors.

#![forbid(unsafe_code)]

pub mod consensus;
pub mod consensus_params;
pub mod error;
pub mod manifest;
pub mod types;

pub use consensus_params::{branch_id_for_height, ConsensusParams};
pub use manifest::{protocol_manifest_hash, protocol_manifest_hash_hex, PROTOCOL_V0_MANIFEST};

pub use types::{
    BlockHash, BlockHeight, BlockRef, CodeHash, ContractId, Execution, Network, StateRoot, TxId,
};
