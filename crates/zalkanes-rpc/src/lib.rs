//! # zalkanes-rpc
//!
//! JSON-RPC 2.0 server exposing Zalkanes node state.
//! See `docs/rpc.md` for the full API.

#![forbid(unsafe_code)]

use anyhow::Result;
use serde::{Deserialize, Serialize};
use zalkanes_core::types::{BlockHeight, ContractId, StateRoot};
use zalkanes_state::MemoryState;

/// Response for `zalkanes_getInfo`.
#[derive(Debug, Serialize, Deserialize)]
pub struct InfoResponse {
    pub protocol_version: u8,
    pub network: String,
    pub indexed_height: Option<BlockHeight>,
    pub state_root: String,
}

/// Response for `zalkanes_getContract`.
#[derive(Debug, Serialize, Deserialize)]
pub struct ContractResponse {
    pub contract_id: String,
    pub code_hash: String,
    pub deployment_height: BlockHeight,
    pub code_size: usize,
}

/// Response for `zalkanes_view`.
#[derive(Debug, Serialize, Deserialize)]
pub struct ViewResponse {
    pub output_hex: String,
    pub fuel_used: u64,
    pub success: bool,
    pub error: Option<String>,
}

/// A minimal in-process RPC handler (used by CLI and testkit).
/// Production: wrap with jsonrpsee server.
pub struct RpcHandler {
    state: std::sync::Arc<std::sync::RwLock<MemoryState>>,
    indexed_height: std::sync::Arc<std::sync::RwLock<Option<BlockHeight>>>,
    network: String,
}

impl RpcHandler {
    pub fn new(
        state: std::sync::Arc<std::sync::RwLock<MemoryState>>,
        indexed_height: std::sync::Arc<std::sync::RwLock<Option<BlockHeight>>>,
        network: String,
    ) -> Self {
        Self {
            state,
            indexed_height,
            network,
        }
    }

    pub fn get_info(&self) -> InfoResponse {
        let state = self.state.read().unwrap();
        let root = state.compute_root();
        let height = *self.indexed_height.read().unwrap();
        InfoResponse {
            protocol_version: 0,
            network: self.network.clone(),
            indexed_height: height,
            state_root: root.as_hex(),
        }
    }

    pub fn get_state_root(&self, _height: Option<BlockHeight>) -> String {
        // In production: look up historical root at given height.
        // For v0 in-memory: return current root.
        self.state.read().unwrap().compute_root().as_hex()
    }

    pub fn get_contract(&self, contract_id_hex: &str) -> Option<ContractResponse> {
        let id_bytes = hex::decode(contract_id_hex).ok()?;
        if id_bytes.len() != 32 {
            return None;
        }
        let mut id = [0u8; 32];
        id.copy_from_slice(&id_bytes);
        let state = self.state.read().unwrap();
        let (code_hash, code) = state.contracts.get(&id)?;
        Some(ContractResponse {
            contract_id: contract_id_hex.to_string(),
            code_hash: code_hash.as_hex(),
            deployment_height: 0, // TODO: track per contract
            code_size: code.len(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, RwLock};

    #[test]
    fn get_info_returns_current_state() {
        let state = Arc::new(RwLock::new(MemoryState::new()));
        let height = Arc::new(RwLock::new(None));
        let handler = RpcHandler::new(state, height, "regtest".to_string());
        let info = handler.get_info();
        assert_eq!(info.protocol_version, 0);
        assert_eq!(info.network, "regtest");
        assert!(info.indexed_height.is_none());
    }
}
