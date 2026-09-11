//! The trusted canonical chain source: our own Zebra node.
//!
//! This is the only authority for freshness, wallet birthday, scanning, and
//! reorg identity. CLI code must obtain [`CanonicalTip`] from here — it must
//! never manufacture one.

#![forbid(unsafe_code)]

use anyhow::{anyhow, bail, Result};
use serde::Deserialize;
use zcash_client_backend::data_api::chain::ChainState;
use zcash_client_backend::proto::service::TreeState;
use zcash_primitives::block::Block;
use zcash_protocol::consensus::Network;

use crate::funding::CanonicalTip;

/// A source of canonical chain data, backed by our own Zebra.
pub trait CanonicalChainSource: crate::funding::TipSource {
    /// The internal block hash at `height`.
    fn block_hash(&self, height: u32) -> Result<[u8; 32]>;
    /// The full Zcash block at `height`.
    fn block(&self, height: u32) -> Result<Block>;
    /// The chain state (commitment-tree frontiers) at the end of `height`.
    fn tree_state(&self, height: u32) -> Result<ChainState>;
}

/// A [`CanonicalChainSource`] implemented against our own Zebra JSON-RPC.
pub struct ZebraCanonicalChainSource {
    rpc_url: String,
    network: Network,
    client: reqwest::blocking::Client,
}

impl ZebraCanonicalChainSource {
    pub fn new(rpc_url: impl Into<String>, network: Network) -> Result<Self> {
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .map_err(|e| anyhow!("build http client: {e}"))?;
        Ok(Self {
            rpc_url: rpc_url.into(),
            network,
            client,
        })
    }

    fn rpc<T: serde::de::DeserializeOwned>(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<T> {
        let body = serde_json::json!({
            "jsonrpc": "1.0",
            "id": "zalkanes",
            "method": method,
            "params": params,
        });
        let resp: RpcResponse<T> = self
            .client
            .post(&self.rpc_url)
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .map_err(|e| anyhow!("rpc {method}: {e}"))?
            .json()
            .map_err(|e| anyhow!("rpc {method} deserialize: {e}"))?;
        resp.result.ok_or_else(|| {
            anyhow!(
                "rpc {method} error: {}",
                resp.error.map(|e| e.message).unwrap_or_default()
            )
        })
    }
}

#[derive(Deserialize)]
struct RpcResponse<T> {
    #[allow(dead_code)]
    result: Option<T>,
    #[allow(dead_code)]
    error: Option<RpcError>,
}

#[derive(Deserialize)]
struct RpcError {
    #[allow(dead_code)]
    message: String,
}

#[derive(Deserialize)]
struct BlockchainInfo {
    blocks: u32,
    #[serde(rename = "bestblockhash")]
    best_block_hash: String,
}

#[derive(Deserialize)]
struct TreestateResponse {
    height: u64,
    hash: String,
    #[serde(default)]
    sapling: Option<TreestateTree>,
    #[serde(default)]
    orchard: Option<TreestateTree>,
    #[serde(default)]
    ironwood: Option<TreestateTree>,
}

#[derive(Deserialize)]
struct TreestateTree {
    commitments: TreestateCommitments,
}

#[derive(Deserialize)]
struct TreestateCommitments {
    #[serde(rename = "finalState")]
    final_state: String,
}

/// Reverse a display-order (byte-reversed) hex hash to internal order.
fn display_hex_to_internal(hex_str: &str) -> Result<[u8; 32]> {
    let hex_str = hex_str.trim_start_matches("0x");
    let mut bytes = hex::decode(hex_str).map_err(|e| anyhow!("invalid hex hash: {e}"))?;
    if bytes.len() != 32 {
        bail!("invalid hash length {}", bytes.len());
    }
    bytes.reverse();
    let mut out = [0u8; 32];
    out.copy_from_slice(&bytes);
    Ok(out)
}

impl crate::funding::TipSource for ZebraCanonicalChainSource {
    fn canonical_tip(&self) -> Result<CanonicalTip> {
        let info: BlockchainInfo = self.rpc("getblockchaininfo", serde_json::json!([]))?;
        Ok(CanonicalTip {
            height: info.blocks,
            hash: display_hex_to_internal(&info.best_block_hash)?,
        })
    }
}

impl CanonicalChainSource for ZebraCanonicalChainSource {
    fn block_hash(&self, height: u32) -> Result<[u8; 32]> {
        let h: String = self.rpc("getblockhash", serde_json::json!([height]))?;
        display_hex_to_internal(&h)
    }

    fn block(&self, height: u32) -> Result<Block> {
        let expected = self.block_hash(height)?;
        let raw: String = self.rpc("getblock", serde_json::json!([height.to_string(), 0]))?;
        let bytes = hex::decode(raw.trim_start_matches("0x"))
            .map_err(|e| anyhow!("invalid block hex: {e}"))?;
        let block = Block::read(&mut &bytes[..], &self.network)
            .map_err(|e| anyhow!("parse block {height}: {e}"))?;
        let actual = block.header().hash().0;
        if actual != expected {
            bail!(
                "block hash mismatch at {height}: expected {}, got {}",
                hex::encode(expected),
                hex::encode(actual)
            );
        }
        Ok(block)
    }

    fn tree_state(&self, height: u32) -> Result<ChainState> {
        let ts: TreestateResponse =
            self.rpc("z_gettreestate", serde_json::json!([height.to_string()]))?;
        let network_name = match self.network {
            Network::MainNetwork => "main",
            Network::TestNetwork => "test",
        };
        let tree_state = TreeState {
            network: network_name.to_string(),
            height: ts.height,
            hash: ts.hash.trim_start_matches("0x").to_string(),
            time: 0,
            sapling_tree: ts
                .sapling
                .map(|t| t.commitments.final_state)
                .unwrap_or_default(),
            orchard_tree: ts
                .orchard
                .map(|t| t.commitments.final_state)
                .unwrap_or_default(),
            ironwood_tree: ts
                .ironwood
                .map(|t| t.commitments.final_state)
                .unwrap_or_default(),
        };
        tree_state
            .to_chain_state()
            .map_err(|e| anyhow!("treestate -> chain state: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_hex_reversal_round_trips() {
        // A known display-order hash reversed to internal order and back.
        let display = "11".repeat(32);
        assert_eq!(display.len(), 64);
        let internal = display_hex_to_internal(&display).unwrap();
        let mut back = internal;
        back.reverse();
        assert_eq!(hex::encode(back), display);
    }

    #[test]
    fn empty_treestate_parses_to_empty_chain_state() {
        // A genesis treestate with empty commitment trees yields empty frontiers.
        let tree_state = TreeState {
            network: "test".to_string(),
            height: 0,
            hash: "00".repeat(32),
            time: 0,
            sapling_tree: String::new(),
            orchard_tree: String::new(),
            ironwood_tree: String::new(),
        };
        let chain_state = tree_state.to_chain_state().unwrap();
        assert_eq!(u32::from(chain_state.block_height()), 0);
    }
}
