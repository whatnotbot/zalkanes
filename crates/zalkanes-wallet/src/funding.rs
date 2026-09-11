//! Funding-source abstraction.
//!
//! A [`FundingSource`] turns a funding-pool-agnostic [`TxRequest`] into a
//! signed, serialized transaction. The ZALK payload (OP_RETURN message and P2SH
//! carrier structure) is produced by the shared helpers in `zalkanes-tx`, so the
//! two funding pools (`transparent` and `shielded`) cannot drift in what they
//! commit on chain.

#![forbid(unsafe_code)]

use anyhow::Result;
use zalkanes_core::types::Network;
use zalkanes_tx::{OutPoint, SignedTx};
use zcash_protocol::consensus::BranchId;

use crate::policy::PrivacyPolicy;

/// A canonical, funding-pool-agnostic description of a Zalkanes transaction.
///
/// The variants mirror the on-chain lifecycle: PREPARE creates carrier UTXOs,
/// DEPLOY spends them and publishes the DEPLOY message, CALL publishes a CALL
/// message. None of these fields mention *how* the transaction is funded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TxRequest {
    /// PREPARE: create one carrier P2SH output per entry in `carrier_values`.
    Prepare {
        /// Value (zat) assigned to each carrier output.
        carrier_values: Vec<u64>,
    },
    /// DEPLOY: spend carrier UTXOs (publishing WASM chunks in their scriptSigs)
    /// and emit the DEPLOY OP_RETURN message.
    Deploy {
        /// WASM chunks, split to `CHUNK_PAYLOAD_SIZE`.
        chunks: Vec<Vec<u8>>,
        /// The carrier UTXOs to spend (created by PREPARE).
        carrier_outpoints: Vec<OutPoint>,
        /// Value of each carrier UTXO.
        carrier_values: Vec<u64>,
        /// The already-encoded Zalkanes DEPLOY message (without the OP_RETURN
        /// wrapper).
        op_return: Vec<u8>,
    },
    /// CALL: emit the CALL OP_RETURN message.
    Call {
        /// The already-encoded Zalkanes CALL message (without the OP_RETURN
        /// wrapper).
        op_return: Vec<u8>,
    },
}

/// Per-transaction context shared by every funding source.
#[derive(Clone, Debug)]
pub struct FundContext {
    /// Which network the transaction targets.
    pub network: Network,
    /// Target height, used to resolve the consensus branch id for signing.
    pub target_height: u32,
    /// The privacy policy the caller has acknowledged.
    pub policy: PrivacyPolicy,
}

impl FundContext {
    /// The consensus branch id active at [`Self::target_height`].
    pub fn branch_id(&self) -> BranchId {
        zalkanes_core::branch_id_for_height(self.network, self.target_height)
    }
}

/// A transparent funding UTXO (outpoint + value).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FundingUtxo {
    pub outpoint: OutPoint,
    pub value: u64,
}

/// A pool of funds that can pay for a Zalkanes transaction.
///
/// Implementations are responsible for selecting inputs, computing change, and
/// signing — but the ZALK payload itself is fixed by [`TxRequest`].
pub trait FundingSource {
    /// Human-readable pool name for logging and UX (`"transparent"` or
    /// `"shielded"`).
    fn pool_name(&self) -> &'static str;

    /// Produce a signed, serialized transaction realizing `request`, funded
    /// from this source.
    fn fund(&self, request: &TxRequest, ctx: &FundContext) -> Result<SignedTx>;
}

/// A `FundingSource` together with the pool-selection preference the user chose.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FundingPool {
    /// Spend transparent UTXOs only.
    Transparent,
    /// Spend shielded (Orchard/Ironwood) notes only.
    Shielded,
    /// Prefer shielded; fall back to transparent if shielded cannot cover.
    Auto,
}

impl FundingPool {
    pub fn as_str(self) -> &'static str {
        match self {
            FundingPool::Transparent => "transparent",
            FundingPool::Shielded => "shielded",
            FundingPool::Auto => "auto",
        }
    }
}

impl std::str::FromStr for FundingPool {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "transparent" => Ok(FundingPool::Transparent),
            "shielded" => Ok(FundingPool::Shielded),
            "auto" => Ok(FundingPool::Auto),
            other => Err(format!(
                "unknown funding pool {other:?}; expected transparent, shielded, or auto"
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn funding_pool_round_trips() {
        for p in [
            FundingPool::Transparent,
            FundingPool::Shielded,
            FundingPool::Auto,
        ] {
            assert_eq!(p.as_str().parse::<FundingPool>().unwrap(), p);
        }
        assert!("bogus".parse::<FundingPool>().is_err());
    }
}
