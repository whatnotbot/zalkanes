//! # zalkanes-rpc
//!
//! JSON-RPC 2.0 server exposing Zalkanes node state.
//! See `docs/rpc.md` for the full API.
//!
//! Backed by an in-memory state handle for v0; reads go through a read-only
//! snapshot. The server never mutates state directly.

#![forbid(unsafe_code)]

use std::net::SocketAddr;
use std::sync::{Arc, RwLock};

use anyhow::Result;
use serde::{Deserialize, Serialize};
use zalkanes_core::types::{BlockHeight, BlockRef, Network};
use zalkanes_state::MemoryState;

/// Response for `zalkanes_getInfo`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InfoResponse {
    pub protocol_version: u8,
    pub network: String,
    pub indexed_height: Option<BlockHeight>,
    pub chain_tip_height: Option<BlockHeight>,
    pub state_root: String,
}

/// Response for `zalkanes_getContract`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContractResponse {
    pub contract_id: String,
    pub code_hash: String,
    pub code_size: usize,
}

/// Response for `zalkanes_view`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ViewResponse {
    pub output_hex: String,
    pub fuel_used: u64,
    pub success: bool,
    pub error: Option<String>,
}

/// Shared, thread-safe node state backing the RPC server.
#[derive(Clone)]
pub struct RpcHandler {
    state: Arc<RwLock<MemoryState>>,
    indexed_height: Arc<RwLock<Option<BlockHeight>>>,
    chain_tip: Arc<RwLock<Option<BlockRef>>>,
    network: Network,
}

impl RpcHandler {
    pub fn new(network: Network) -> Self {
        Self {
            state: Arc::new(RwLock::new(MemoryState::new())),
            indexed_height: Arc::new(RwLock::new(None)),
            chain_tip: Arc::new(RwLock::new(None)),
            network,
        }
    }

    /// Shared handles for an indexer to feed in height + tip as it syncs.
    pub fn state_handle(&self) -> Arc<RwLock<MemoryState>> {
        self.state.clone()
    }

    pub fn indexed_height_handle(&self) -> Arc<RwLock<Option<BlockHeight>>> {
        self.indexed_height.clone()
    }

    pub fn chain_tip_handle(&self) -> Arc<RwLock<Option<BlockRef>>> {
        self.chain_tip.clone()
    }

    pub fn network(&self) -> Network {
        self.network
    }

    pub fn get_info(&self) -> InfoResponse {
        let state = self.state.read().expect("state lock poisoned");
        let root = state.compute_root();
        let indexed_height = *self.indexed_height.read().expect("height lock poisoned");
        let chain_tip_height = self
            .chain_tip
            .read()
            .expect("tip lock poisoned")
            .map(|r| r.height);
        InfoResponse {
            protocol_version: 0,
            network: self.network.zebra_name().to_string(),
            indexed_height,
            chain_tip_height,
            state_root: root.as_hex(),
        }
    }

    pub fn get_state_root(&self) -> String {
        self.state
            .read()
            .expect("state lock poisoned")
            .compute_root()
            .as_hex()
    }

    pub fn get_contract(&self, contract_id_hex: &str) -> Option<ContractResponse> {
        let id_bytes = hex::decode(contract_id_hex).ok()?;
        if id_bytes.len() != 32 {
            return None;
        }
        let mut id = [0u8; 32];
        id.copy_from_slice(&id_bytes);
        let state = self.state.read().expect("state lock poisoned");
        let (code_hash, code) = state.contracts.get(&id)?;
        Some(ContractResponse {
            contract_id: contract_id_hex.to_string(),
            code_hash: code_hash.as_hex(),
            code_size: code.len(),
        })
    }
}

/// Start the JSON-RPC server and return a handle that resolves when it stops.
pub async fn serve(addr: SocketAddr, handler: RpcHandler) -> Result<()> {
    use jsonrpsee::core::RpcResult;
    use jsonrpsee::server::Server;
    use jsonrpsee::types::Params;
    use jsonrpsee::RpcModule;

    let server = Server::builder().build(addr).await?;
    let mut module = RpcModule::new(handler);

    module.register_method("zalkanes_getInfo", |_params: Params, ctx, _ext| {
        let h: &RpcHandler = ctx;
        let info = h.get_info();
        Ok::<_, jsonrpsee::types::ErrorObjectOwned>(info)
    })?;

    module.register_method("zalkanes_getStateRoot", |_params: Params, ctx, _ext| {
        let h: &RpcHandler = ctx;
        let root = h.get_state_root();
        Ok::<_, jsonrpsee::types::ErrorObjectOwned>(root)
    })?;

    module.register_method(
        "zalkanes_getContract",
        |params: Params, ctx, _ext| -> RpcResult<Option<ContractResponse>> {
            let h: &RpcHandler = ctx;
            let id: String = params.one()?;
            Ok(h.get_contract(&id))
        },
    )?;

    let handle = server.start(module);
    handle.stopped().await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_info_returns_current_state() {
        let handler = RpcHandler::new(Network::Regtest);
        let info = handler.get_info();
        assert_eq!(info.protocol_version, 0);
        assert_eq!(info.network, "regtest");
        assert!(info.indexed_height.is_none());
        assert!(info.chain_tip_height.is_none());
    }

    #[test]
    fn chain_tip_is_reported() {
        let handler = RpcHandler::new(Network::Mainnet);
        *handler.chain_tip_handle().write().unwrap() = Some(BlockRef {
            height: 3_478_274,
            hash: zalkanes_core::types::BlockHash([0u8; 32]),
        });
        let info = handler.get_info();
        assert_eq!(info.chain_tip_height, Some(3_478_274));
        assert_eq!(info.network, "main");
    }
}
