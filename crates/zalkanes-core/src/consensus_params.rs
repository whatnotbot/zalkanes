//! Network consensus parameters and height-aware branch-id resolution.
//!
//! Bridges Zalkanes' [`Network`] to librustzcash's canonical
//! [`zcash_protocol::consensus::Parameters`] implementations, and resolves the
//! consensus branch id active at a given block height via the canonical
//! [`zcash_protocol::consensus::BranchId::for_height`].
//!
//! **Consensus-critical.** The branch id commits to which network-upgrade rules
//! validate a transaction; signing for the wrong epoch makes every transparent
//! signature invalid.

#![forbid(unsafe_code)]

use zcash_protocol::consensus::{
    self, BlockHeight, BranchId, MainNetwork, NetworkUpgrade, Parameters, TestNetwork,
};

use crate::types::Network;

/// Network upgrade parameters for a Zalkanes network.
///
/// Mainnet and Testnet delegate to librustzcash's canonical
/// [`MainNetwork`]/[`TestNetwork`]. Regtest is a custom network where every
/// supported upgrade activates from height 1 (matching the configured Zebra
/// regtest chain).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConsensusParams {
    Main,
    Test,
    Regtest,
}

impl ConsensusParams {
    /// The [`ConsensusParams`] for a Zalkanes [`Network`].
    pub fn for_network(network: Network) -> Self {
        match network {
            Network::Mainnet => ConsensusParams::Main,
            Network::Testnet => ConsensusParams::Test,
            Network::Regtest => ConsensusParams::Regtest,
        }
    }
}

impl Parameters for ConsensusParams {
    fn network_type(&self) -> consensus::NetworkType {
        match self {
            ConsensusParams::Main => consensus::NetworkType::Main,
            ConsensusParams::Test => consensus::NetworkType::Test,
            ConsensusParams::Regtest => consensus::NetworkType::Regtest,
        }
    }

    fn activation_height(&self, nu: NetworkUpgrade) -> Option<BlockHeight> {
        match self {
            ConsensusParams::Main => MainNetwork.activation_height(nu),
            ConsensusParams::Test => TestNetwork.activation_height(nu),
            // Regtest: all upgrades active from height 1 (matches the configured
            // Zebra regtest chain).
            ConsensusParams::Regtest => Some(BlockHeight::from_u32(1)),
        }
    }
}

/// The consensus branch id active at `height` on `network`.
///
/// Uses the canonical [`BranchId::for_height`] over the network's consensus
/// parameters, so it is automatically height-aware for every network upgrade the
/// pinned librustzcash supports (Overwinter … Nu6.3). Signing a transaction with
/// the wrong epoch's branch id makes it invalid under consensus.
pub fn branch_id_for_height(network: Network, height: u32) -> BranchId {
    let params = ConsensusParams::for_network(network);
    BranchId::for_height(&params, BlockHeight::from_u32(height))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regtest_all_upgrades_active_from_height_1() {
        let p = ConsensusParams::Regtest;
        assert!(p.is_nu_active(NetworkUpgrade::Nu6_3, BlockHeight::from_u32(1)));
        assert_eq!(branch_id_for_height(Network::Regtest, 1), BranchId::Nu6_3);
    }

    #[test]
    fn mainnet_boundaries_use_canonical_params() {
        // Immediately before Nu5 activation (mainnet 1_687_104).
        assert_eq!(
            branch_id_for_height(Network::Mainnet, 1_687_103),
            BranchId::Canopy
        );
        assert_eq!(
            branch_id_for_height(Network::Mainnet, 1_687_104),
            BranchId::Nu5
        );
        // Nu6 activation (mainnet 2_726_400).
        assert_eq!(
            branch_id_for_height(Network::Mainnet, 2_726_399),
            BranchId::Nu5
        );
        assert_eq!(
            branch_id_for_height(Network::Mainnet, 2_726_400),
            BranchId::Nu6
        );
    }

    #[test]
    fn testnet_is_nu6_3_at_tip() {
        assert_eq!(
            branch_id_for_height(Network::Testnet, 4_340_000),
            BranchId::Nu6_3
        );
    }
}
