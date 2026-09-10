//! Real Zcash block + transaction parsing via librustzcash.
//!
//! This is the ONLY way production indexing obtains Zalkanes messages: it
//! deserializes canonical Zcash blocks (`zcash_primitives::block::Block`),
//! reads transparent transaction OP_RETURN outputs and P2SH scriptSigs, and
//! extracts DEPLOY/CALL messages and carrier chunks.
//!
//! No `TestTx`, no synthetic blocks. See `docs/architecture.md`.

#![forbid(unsafe_code)]

use anyhow::{bail, Context, Result};
use zalkanes_core::types::{BlockHash, BlockHeight, Network, TxId};
use zcash_protocol::consensus::{self, MainNetwork, TestNetwork};

/// A parsed Zcash block, reduced to the data Zalkanes cares about.
#[derive(Debug, Clone)]
pub struct ParsedBlock {
    pub height: BlockHeight,
    pub hash: BlockHash,
    /// All transactions in consensus order (coinbase first).
    pub transactions: Vec<ParsedTransaction>,
}

/// A parsed transparent transaction.
#[derive(Debug, Clone)]
pub struct ParsedTransaction {
    /// ZIP-244 / legacy txid in internal byte order (as stored in blocks).
    pub txid: TxId,
    /// (output_index, script_pubkey) for every transparent output.
    pub outputs: Vec<(u16, Vec<u8>)>,
    /// (input_index, script_sig) for every transparent input.
    pub inputs: Vec<(u32, Vec<u8>)>,
}

/// Network parameters for `Block::read`, bridged to librustzcash.
fn consensus_params(network: Network) -> Result<ConsensusParams> {
    Ok(match network {
        Network::Mainnet => ConsensusParams::Main,
        Network::Testnet => ConsensusParams::Test,
        // librustzcash has no RegtestNetwork marker struct; regtest blocks use
        // the same branch IDs as testnet at the equivalent heights. We use a
        // custom params object (see below).
        Network::Regtest => ConsensusParams::Regtest,
    })
}

#[derive(Clone)]
enum ConsensusParams {
    Main,
    Test,
    Regtest,
}

impl consensus::Parameters for ConsensusParams {
    fn network_type(&self) -> consensus::NetworkType {
        match self {
            ConsensusParams::Main => consensus::NetworkType::Main,
            ConsensusParams::Test => consensus::NetworkType::Test,
            ConsensusParams::Regtest => consensus::NetworkType::Regtest,
        }
    }

    fn activation_height(&self, nu: consensus::NetworkUpgrade) -> Option<consensus::BlockHeight> {
        match self {
            ConsensusParams::Main => MainNetwork.activation_height(nu),
            ConsensusParams::Test => TestNetwork.activation_height(nu),
            // Regtest: all upgrades active from height 1.
            ConsensusParams::Regtest => Some(consensus::BlockHeight::from_u32(1)),
        }
    }
}

/// Parse a raw Zcash block into Zalkanes-relevant data.
///
/// `height` and `hash` are supplied by the caller (from the chain source) and
/// are used for ContractId derivation and execution records. The parser
/// independently reads the coinbase-claimed height for sanity.
pub fn parse_block(
    raw: &[u8],
    height: BlockHeight,
    hash: BlockHash,
    network: Network,
) -> Result<ParsedBlock> {
    let params = consensus_params(network)?;
    let block = zcash_primitives::block::Block::read(raw, &params)
        .context("failed to deserialize Zcash block")?;

    // Sanity: coinbase height must agree with the caller's height.
    let claimed: u32 = block.claimed_height().into();
    if claimed != height {
        bail!("block height mismatch: chain source said {height}, coinbase claims {claimed}");
    }

    let mut transactions = Vec::with_capacity(block.vtx().len());
    for tx in block.vtx().iter() {
        // txid in internal byte order.
        let txid: [u8; 32] = tx.txid().into();
        let txid = TxId(txid);

        let mut outputs = Vec::new();
        let mut inputs = Vec::new();

        if let Some(bundle) = tx.transparent_bundle() {
            for (idx, txout) in bundle.vout.iter().enumerate() {
                outputs.push((idx as u16, txout.script_pubkey().0 .0.clone()));
            }
            for (idx, txin) in bundle.vin.iter().enumerate() {
                inputs.push((idx as u32, txin.script_sig().0 .0.clone()));
            }
        }

        transactions.push(ParsedTransaction {
            txid,
            outputs,
            inputs,
        });
    }

    Ok(ParsedBlock {
        height,
        hash,
        transactions,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use zcash_protocol::consensus::Parameters;

    #[test]
    fn regtest_params_activate_all_upgrades() {
        let p = ConsensusParams::Regtest;
        assert!(p.is_nu_active(
            consensus::NetworkUpgrade::Nu6,
            consensus::BlockHeight::from_u32(1)
        ));
    }

    #[test]
    fn mainnet_params_match_librustzcash() {
        let p = ConsensusParams::Main;
        assert_eq!(
            p.activation_height(consensus::NetworkUpgrade::Sapling),
            MainNetwork.activation_height(consensus::NetworkUpgrade::Sapling)
        );
    }
}
