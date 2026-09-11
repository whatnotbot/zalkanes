//! Funding-source abstraction.
//!
//! A [`FundingSource`] turns a funding-pool-agnostic [`TxRequest`] into a
//! [`FundingPlan`] — an inspected, not-yet-authorized description of the
//! transaction. Authorization (proving, signing, extracting) is a separate
//! layer, so callers can `--dry-run` inspect a plan without generating proofs
//! or signatures.

#![forbid(unsafe_code)]

use anyhow::{bail, Result};
use zalkanes_core::types::Network;
use zalkanes_tx::OutPoint;
use zcash_protocol::consensus::BranchId;

use crate::plan::FundingPlan;

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

impl TxRequest {
    /// Short human-readable kind name.
    pub fn kind(&self) -> &'static str {
        match self {
            TxRequest::Prepare { .. } => "prepare",
            TxRequest::Deploy { .. } => "deploy",
            TxRequest::Call { .. } => "call",
        }
    }

    /// The encoded ZALK OP_RETURN payload (the bytes that go on chain inside the
    /// OP_RETURN wrapper), if this request has one. PREPARE has none.
    pub fn op_return_payload(&self) -> Option<&[u8]> {
        match self {
            TxRequest::Prepare { .. } => None,
            TxRequest::Deploy { op_return, .. } | TxRequest::Call { op_return } => Some(op_return),
        }
    }

    /// The first byte of the ZALK payload (the message type), if present.
    pub fn opcode(&self) -> Option<u8> {
        self.op_return_payload().and_then(|p| p.first().copied())
    }
}

/// The canonical chain identity a wallet plan is pinned to: a height **and** a
/// block hash. A same-height reorg is detectable only by comparing the hash.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CanonicalTip {
    pub height: u32,
    pub hash: [u8; 32],
}

/// A source of the authoritative canonical tip. Public authorization stages
/// take this (never a caller-manufactured [`CanonicalTip`]), so freshness is
/// always queried from the trusted chain source.
pub trait TipSource {
    fn canonical_tip(&self) -> Result<CanonicalTip>;
}

/// Per-transaction context shared by every funding source.
#[derive(Clone, Copy, Debug)]
pub struct FundContext {
    /// Which network the transaction targets.
    pub network: Network,
    /// The canonical chain tip (an existing block with a known height + hash)
    /// the plan is pinned to.
    pub chain_tip: CanonicalTip,
    /// The height whose consensus rules / expiry / witness-selection semantics
    /// the new transaction is built for (normally `chain_tip.height + 1`).
    pub target_height: u32,
}

impl FundContext {
    /// Construct a context where the transaction targets the block immediately
    /// after the canonical tip.
    pub fn new(network: Network, chain_tip: CanonicalTip) -> Self {
        Self {
            network,
            chain_tip,
            target_height: chain_tip.height + 1,
        }
    }

    /// The canonical tip this context is pinned to.
    pub fn canonical_tip(&self) -> CanonicalTip {
        self.chain_tip
    }

    /// Enforce the intended relationship: the transaction target height is the
    /// block after the canonical tip.
    pub fn validate(&self) -> Result<()> {
        if self.target_height != self.chain_tip.height + 1 {
            bail!(
                "target_height {} must be chain_tip.height + 1 ({})",
                self.target_height,
                self.chain_tip.height + 1
            );
        }
        Ok(())
    }

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

/// A pool of funds that can plan a Zalkanes transaction.
///
/// `plan` performs **no** proving and **no** signing: it only selects inputs,
/// computes change/fee, and assembles the (inspectable) transaction shape. The
/// returned [`FundingPlan`] is authorized separately.
pub trait FundingSource {
    /// Human-readable pool name for logging and UX (`"transparent"` or
    /// `"shielded"`).
    fn pool_name(&self) -> &'static str;

    /// Plan a transaction realizing `request`, funded from this source.
    ///
    /// Produces no proofs or signatures. Returns a [`FundingPlan`] that can be
    /// inspected (`--dry-run`) and then authorized in stages.
    fn plan(&self, request: &TxRequest, ctx: &FundContext) -> Result<FundingPlan>;
}

/// A `FundingSource` together with the pool-selection preference the user chose.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FundingPool {
    /// Spend transparent UTXOs only.
    Transparent,
    /// Spend shielded (Orchard/Ironwood) notes only, for the stages capable of
    /// shielded funding (PREPARE and CALL).
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

    #[test]
    fn tx_request_opcode() {
        let call = TxRequest::Call {
            op_return: vec![0x02, 0x00],
        };
        assert_eq!(call.opcode(), Some(0x02));
        assert_eq!(call.kind(), "call");

        let prepare = TxRequest::Prepare {
            carrier_values: vec![1],
        };
        assert_eq!(prepare.opcode(), None);
        assert_eq!(prepare.kind(), "prepare");
    }

    #[test]
    fn branch_selection_uses_target_height_not_chain_tip() {
        use zcash_protocol::consensus::{NetworkUpgrade, Parameters, TestNetwork};
        // Find a real testnet upgrade boundary (Nu6_3).
        let nu6_3 = TestNetwork
            .activation_height(NetworkUpgrade::Nu6_3)
            .map(u32::from)
            .unwrap();

        // chain_tip is one block BEFORE the boundary; target_height is AT it.
        let ctx = FundContext::new(
            Network::Testnet,
            CanonicalTip {
                height: nu6_3 - 1,
                hash: [0u8; 32],
            },
        );
        assert_eq!(ctx.target_height, nu6_3);

        // The branch must be resolved from target_height, not chain_tip.height.
        let expected = zalkanes_core::branch_id_for_height(Network::Testnet, nu6_3);
        let from_tip = zalkanes_core::branch_id_for_height(Network::Testnet, nu6_3 - 1);
        assert_eq!(ctx.branch_id(), expected);
        assert_ne!(ctx.branch_id(), from_tip);
    }
}
