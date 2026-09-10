//! # zalkanes-chain
//!
//! `ChainSource` trait + `RpcChainSource` — the only production chain source.
//! Points to a Zcash JSON-RPC endpoint over HTTP(S).
//!
//! Supported upstreams (any Bitcoin/Zcash-compatible JSON-RPC):
//! - Local Zebra node (consensus-validated; recommended for mainnet)
//! - Local zcashd node
//! - Hosted providers such as NOWNodes (`https://zec.nownodes.io/<api_key>`)

#![forbid(unsafe_code)]

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use zalkanes_core::types::{BlockHash, BlockHeight, BlockRef, Network};

/// Abstraction over a source of validated Zcash block data.
///
/// Production implementation: [`RpcChainSource`].
/// Test implementation: `MockChainSource` in zalkanes-testkit.
pub trait ChainSource: Send + Sync {
    fn network(&self) -> Network;
    fn tip(&self) -> Result<BlockRef>;
    fn block_hash(&self, height: BlockHeight) -> Result<BlockHash>;
    fn raw_block(&self, hash: &BlockHash) -> Result<Vec<u8>>;
}

/// Connects to a Zcash JSON-RPC endpoint over HTTP(S).
///
/// The endpoint may be a Zebra node, zcashd, or a hosted provider (NOWNodes).
/// Hosted providers are used for development convenience only; mainnet trust
/// is anchored on a locally operated, consensus-validating Zebra node.
pub struct RpcChainSource {
    rpc_url: String,
    api_key: Option<String>,
    network: Network,
    client: reqwest::blocking::Client,
}

// Backwards-compatible alias.
pub type ZebraRpcChainSource = RpcChainSource;

impl RpcChainSource {
    /// Build a chain source for a bare RPC URL (no auth).
    pub fn new(rpc_url: impl Into<String>, network: Network) -> Result<Self> {
        Self::builder(rpc_url, network).build()
    }

    pub fn builder(rpc_url: impl Into<String>, network: Network) -> RpcChainSourceBuilder {
        RpcChainSourceBuilder {
            rpc_url: rpc_url.into(),
            api_key: None,
            network,
        }
    }

    /// Validate connection: check network and basic reachability.
    pub fn validate(&self) -> Result<()> {
        let info = self.get_blockchain_info()?;
        if info.chain != self.network.zebra_name() {
            bail!(
                "RPC reports network {:?} but expected {:?}",
                info.chain,
                self.network.zebra_name()
            );
        }
        tracing::info!(
            chain = info.chain,
            blocks = info.blocks,
            "chain source validated"
        );
        Ok(())
    }

    fn rpc_call<T: for<'de> Deserialize<'de>>(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<T> {
        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": method,
            "params": params,
        });

        let mut req = self
            .client
            .post(&self.rpc_url)
            .header("Content-Type", "application/json");
        if let Some(key) = &self.api_key {
            req = req.header("api-key", key);
        }

        let resp = req
            .json(&body)
            .send()
            .with_context(|| format!("RPC call to {method} failed"))?;

        let rpc_resp: RpcResponse<T> = resp.json().context("failed to deserialize RPC response")?;

        if let Some(err) = rpc_resp.error {
            bail!("RPC error {}: {}", err.code, err.message);
        }

        rpc_resp.result.context("RPC returned null result")
    }

    fn get_blockchain_info(&self) -> Result<BlockchainInfo> {
        self.rpc_call("getblockchaininfo", serde_json::json!([]))
    }
}

pub struct RpcChainSourceBuilder {
    rpc_url: String,
    api_key: Option<String>,
    network: Network,
}

impl RpcChainSourceBuilder {
    /// Set an `api-key` HTTP header (hosted providers such as NOWNodes).
    pub fn api_key(mut self, key: impl Into<String>) -> Self {
        self.api_key = Some(key.into());
        self
    }

    pub fn build(self) -> Result<RpcChainSource> {
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .context("failed to build HTTP client")?;
        Ok(RpcChainSource {
            rpc_url: self.rpc_url,
            api_key: self.api_key,
            network: self.network,
            client,
        })
    }
}

impl ChainSource for RpcChainSource {
    fn network(&self) -> Network {
        self.network
    }

    fn tip(&self) -> Result<BlockRef> {
        let info = self.get_blockchain_info()?;
        let hash_bytes = hex::decode(&info.bestblockhash).context("invalid bestblockhash hex")?;
        let mut hash = [0u8; 32];
        hash.copy_from_slice(&hash_bytes);
        Ok(BlockRef {
            height: info.blocks,
            hash: BlockHash(hash),
        })
    }

    fn block_hash(&self, height: BlockHeight) -> Result<BlockHash> {
        let hash_hex: String = self.rpc_call("getblockhash", serde_json::json!([height]))?;
        let bytes = hex::decode(&hash_hex).context("invalid block hash hex")?;
        let mut hash = [0u8; 32];
        hash.copy_from_slice(&bytes);
        Ok(BlockHash(hash))
    }

    fn raw_block(&self, hash: &BlockHash) -> Result<Vec<u8>> {
        // verbosity=0 returns raw hex-encoded block bytes
        let hex_str: String =
            self.rpc_call("getblock", serde_json::json!([hex::encode(hash.0), 0]))?;
        hex::decode(&hex_str).context("invalid raw block hex")
    }
}

// ── RPC response types ──────────────────────────────────────────────────────

#[derive(Deserialize)]
struct RpcResponse<T> {
    result: Option<T>,
    error: Option<RpcError>,
}

#[derive(Deserialize)]
struct RpcError {
    code: i64,
    message: String,
}

#[derive(Deserialize)]
struct BlockchainInfo {
    chain: String,
    blocks: u32,
    bestblockhash: String,
}
