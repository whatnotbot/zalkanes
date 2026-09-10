//! # zalkanes-rpc
//!
//! JSON-RPC 2.0 server exposing Zalkanes node state.
//!
//! The handler reads through a shared [`StateStore`] handle — the SAME backend
//! the indexer writes to. It never creates its own unrelated state.
//!
//! Methods:
//! - `zalkanes_getInfo` — indexed height, chain tip, state root, syncing flag
//! - `zalkanes_getStateRoot` — authoritative current state root
//! - `zalkanes_getContract` — contract metadata by ContractId
//! - `zalkanes_getCode` — raw WASM hex by ContractId
//! - `zalkanes_view` — read-only call against current indexed state
//! - `zalkanes_getExecution` — execution record by txid
//! - `zalkanes_getBlockExecutions` — executions at a height

#![forbid(unsafe_code)]

use std::net::SocketAddr;
use std::sync::{Arc, RwLock};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use zalkanes_core::types::{
    BlockHash, BlockHeight, BlockRef, ContractId, Execution, Network, TxId,
};
use zalkanes_runtime::{execute, CallContext, CallResult};
use zalkanes_state::StateStore;

/// Shared handle to the authoritative state backend.
pub type SharedState = Arc<RwLock<Box<dyn StateStore>>>;

/// Response for `zalkanes_getInfo`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InfoResponse {
    pub protocol_version: u8,
    pub protocol_manifest_hash: String,
    pub network: String,
    pub indexed_height: Option<BlockHeight>,
    pub chain_tip_height: Option<BlockHeight>,
    pub indexed_block_hash: Option<String>,
    pub state_root: String,
    pub syncing: bool,
}

/// Response for `zalkanes_getContract`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContractResponse {
    pub contract_id: String,
    pub code_hash: String,
    pub code_size: usize,
}

/// Response for `zalkanes_getCode`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeResponse {
    pub contract_id: String,
    pub code_hash: String,
    pub code_hex: String,
}

/// Response for `zalkanes_view`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ViewResponse {
    pub success: bool,
    pub output_hex: String,
    pub fuel_used: u64,
    pub indexed_height: Option<BlockHeight>,
    pub state_root: String,
    pub error: Option<String>,
}

/// Shared, thread-safe node state backing the RPC server.
#[derive(Clone)]
pub struct RpcHandler {
    state: SharedState,
    chain_tip: Arc<RwLock<Option<BlockRef>>>,
    network: Network,
}

impl RpcHandler {
    /// Build a handler from the SAME state handle the indexer uses.
    pub fn new(network: Network, state: SharedState) -> Self {
        Self {
            state,
            chain_tip: Arc::new(RwLock::new(None)),
            network,
        }
    }

    pub fn chain_tip_handle(&self) -> Arc<RwLock<Option<BlockRef>>> {
        self.chain_tip.clone()
    }

    pub fn network(&self) -> Network {
        self.network
    }

    fn with_state<T>(&self, f: impl FnOnce(&dyn StateStore) -> T) -> T {
        let guard = self.state.read().expect("state lock poisoned");
        f(&**guard)
    }

    pub fn get_info(&self) -> InfoResponse {
        let (root, indexed_height, indexed_block_hash) =
            self.with_state(|s| (s.compute_root(), s.indexed_height(), s.indexed_block_hash()));
        let chain_tip_height = self
            .chain_tip
            .read()
            .expect("tip lock poisoned")
            .map(|r| r.height);
        let syncing = match (indexed_height, chain_tip_height) {
            (Some(i), Some(t)) => i < t,
            _ => true,
        };
        InfoResponse {
            protocol_version: 0,
            protocol_manifest_hash: zalkanes_core::protocol_manifest_hash_hex(),
            network: self.network.zebra_name().to_string(),
            indexed_height,
            chain_tip_height,
            indexed_block_hash: indexed_block_hash.map(|h| hex::encode(h.0)),
            state_root: root.as_hex(),
            syncing,
        }
    }

    pub fn get_state_root(&self) -> String {
        self.with_state(|s| s.compute_root().as_hex())
    }

    pub fn get_contract(&self, contract_id_hex: &str) -> Option<ContractResponse> {
        let id = parse_contract_id(contract_id_hex)?;
        self.with_state(|s| {
            s.get_contract(&id)
                .map(|(code_hash, code)| ContractResponse {
                    contract_id: contract_id_hex.to_string(),
                    code_hash: code_hash.as_hex(),
                    code_size: code.len(),
                })
        })
    }

    pub fn get_code(&self, contract_id_hex: &str) -> Option<CodeResponse> {
        let id = parse_contract_id(contract_id_hex)?;
        self.with_state(|s| {
            s.get_contract(&id).map(|(code_hash, code)| CodeResponse {
                contract_id: contract_id_hex.to_string(),
                code_hash: code_hash.as_hex(),
                code_hex: hex::encode(code),
            })
        })
    }

    /// Read-only execution against current indexed state. Writes are discarded.
    pub fn view(&self, contract_id_hex: &str, opcode: u16, input: &[u8]) -> ViewResponse {
        let id = match parse_contract_id(contract_id_hex) {
            Some(id) => id,
            None => {
                return ViewResponse {
                    success: false,
                    output_hex: String::new(),
                    fuel_used: 0,
                    indexed_height: None,
                    state_root: self.get_state_root(),
                    error: Some("invalid contract id".to_string()),
                }
            }
        };

        let indexed_height = self.with_state(|s| s.indexed_height());
        let state_root = self.get_state_root();

        // Execute against a read-only snapshot of the store. Because `execute`
        // only reads through `&dyn StateStore` and buffers writes, we can run it
        // directly under the read lock and discard the returned writes.
        let result = self.with_state(|s| {
            let ctx = CallContext {
                contract_id: id,
                caller: None,
                txid: TxId([0u8; 32]),
                block_height: s.indexed_height().unwrap_or(0),
                opcode,
                input: input.to_vec(),
                fuel_limit: zalkanes_core::consensus::MAX_FUEL_PER_CALL,
                depth: 0,
            };
            execute(ctx, s)
        });

        match result {
            CallResult::Success {
                output, fuel_used, ..
            } => ViewResponse {
                success: true,
                output_hex: hex::encode(output),
                fuel_used,
                indexed_height,
                state_root,
                error: None,
            },
            CallResult::Trap { reason, fuel_used } => ViewResponse {
                success: false,
                output_hex: String::new(),
                fuel_used,
                indexed_height,
                state_root,
                error: Some(reason),
            },
            CallResult::FuelExhausted { fuel_used } => ViewResponse {
                success: false,
                output_hex: String::new(),
                fuel_used,
                indexed_height,
                state_root,
                error: Some("fuel exhausted".to_string()),
            },
            CallResult::InvalidModule { reason } => ViewResponse {
                success: false,
                output_hex: String::new(),
                fuel_used: 0,
                indexed_height,
                state_root,
                error: Some(reason),
            },
            CallResult::ContractNotFound => ViewResponse {
                success: false,
                output_hex: String::new(),
                fuel_used: 0,
                indexed_height,
                state_root,
                error: Some("contract not found".to_string()),
            },
        }
    }

    /// Look up an execution record by txid.
    ///
    /// Accepts the txid in display (byte-reversed) order — the same form
    /// returned by `sendrawtransaction` and block explorers.
    pub fn get_execution(&self, txid_hex: &str) -> Option<Execution> {
        let mut bytes = hex::decode(txid_hex).ok()?;
        if bytes.len() != 32 {
            return None;
        }
        bytes.reverse(); // display order → internal order
        let mut id = [0u8; 32];
        id.copy_from_slice(&bytes);
        let txid = TxId(id);
        self.with_state(|s| s.execution(&txid))
    }

    pub fn get_block_executions(&self, height: BlockHeight) -> Vec<Execution> {
        self.with_state(|s| s.executions_at_height(height))
    }

    pub fn get_indexed_block_hash(&self) -> Option<BlockHash> {
        self.with_state(|s| s.indexed_block_hash())
    }
}

fn parse_contract_id(s: &str) -> Option<ContractId> {
    let bytes = hex::decode(s).ok()?;
    if bytes.len() != 32 {
        return None;
    }
    let mut id = [0u8; 32];
    id.copy_from_slice(&bytes);
    Some(ContractId(id))
}

/// Readiness state for the health endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthState {
    /// Process is alive but not yet connected to the chain source.
    Starting,
    /// Connected and indexing (behind the chain tip).
    Syncing,
    /// Indexer has caught up to the chain tip.
    CaughtUp,
    /// Database is unhealthy (e.g. failed to open) — 503.
    Unhealthy,
}

/// Start a minimal HTTP health/readiness endpoint on `addr`.
///
/// Separate from the JSON-RPC server, for Railway / process supervisors:
/// - `/health` → process alive (200 while up)
/// - `/ready`  → 200 caught up; 202 syncing; 503 unhealthy
pub async fn serve_health(addr: SocketAddr, handler: RpcHandler) -> Result<()> {
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .context("bind health listener")?;
    tracing::info!(%addr, "health/readiness endpoint listening");

    loop {
        let (socket, _) = listener.accept().await.context("accept")?;
        let handler = handler.clone();
        tokio::spawn(async move {
            let _ = handle_health_conn(socket, handler).await;
        });
    }
}

async fn handle_health_conn(mut socket: tokio::net::TcpStream, handler: RpcHandler) -> Result<()> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let mut buf = [0u8; 1024];
    let n = socket.read(&mut buf).await?;
    let request = String::from_utf8_lossy(&buf[..n]).to_string();
    let path = request
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .unwrap_or("/");

    let (status, body) = match path {
        "/health" => ("200 OK", "alive\n"),
        "/ready" => match readiness(&handler) {
            HealthState::CaughtUp => ("200 OK", "caught_up\n"),
            HealthState::Syncing => ("202 Accepted", "syncing\n"),
            HealthState::Starting => ("200 OK", "starting\n"),
            HealthState::Unhealthy => ("503 Service Unavailable", "unhealthy\n"),
        },
        _ => ("404 Not Found", "not_found\n"),
    };

    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    socket.write_all(response.as_bytes()).await?;
    socket.shutdown().await?;
    Ok(())
}

fn readiness(handler: &RpcHandler) -> HealthState {
    let info = handler.get_info();
    match (info.indexed_height, info.chain_tip_height) {
        (Some(i), Some(t)) if i >= t => HealthState::CaughtUp,
        (Some(_), Some(_)) => HealthState::Syncing,
        (Some(_), None) => HealthState::Syncing,
        _ => HealthState::Starting,
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

    module.register_method("zalkanes_getInfo", |_p: Params, ctx, _ext| {
        let h: &RpcHandler = ctx;
        Ok::<_, jsonrpsee::types::ErrorObjectOwned>(h.get_info())
    })?;

    module.register_method("zalkanes_getStateRoot", |_p: Params, ctx, _ext| {
        let h: &RpcHandler = ctx;
        Ok::<_, jsonrpsee::types::ErrorObjectOwned>(h.get_state_root())
    })?;

    module.register_method(
        "zalkanes_getContract",
        |params: Params, ctx, _ext| -> RpcResult<Option<ContractResponse>> {
            let h: &RpcHandler = ctx;
            let id: String = params.one()?;
            Ok(h.get_contract(&id))
        },
    )?;

    module.register_method(
        "zalkanes_getCode",
        |params: Params, ctx, _ext| -> RpcResult<Option<CodeResponse>> {
            let h: &RpcHandler = ctx;
            let id: String = params.one()?;
            Ok(h.get_code(&id))
        },
    )?;

    module.register_method(
        "zalkanes_view",
        |params: Params, ctx, _ext| -> RpcResult<ViewResponse> {
            let h: &RpcHandler = ctx;
            let arr: Vec<serde_json::Value> = params.parse()?;
            let id = arr.first().and_then(|v| v.as_str()).unwrap_or("");
            let opcode = arr.get(1).and_then(|v| v.as_u64()).unwrap_or(0) as u16;
            let input = arr
                .get(2)
                .and_then(|v| v.as_str())
                .map(|s| hex::decode(s).unwrap_or_default())
                .unwrap_or_default();
            Ok(h.view(id, opcode, &input))
        },
    )?;

    module.register_method(
        "zalkanes_getExecution",
        |params: Params, ctx, _ext| -> RpcResult<Option<Execution>> {
            let h: &RpcHandler = ctx;
            let id: String = params.one()?;
            Ok(h.get_execution(&id))
        },
    )?;

    module.register_method(
        "zalkanes_getBlockExecutions",
        |params: Params, ctx, _ext| -> RpcResult<Vec<Execution>> {
            let h: &RpcHandler = ctx;
            let height: u32 = params.one()?;
            Ok(h.get_block_executions(height))
        },
    )?;

    let handle = server.start(module);
    handle.stopped().await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use zalkanes_state::MemoryState;

    fn test_handler() -> RpcHandler {
        let state: SharedState = Arc::new(RwLock::new(Box::new(MemoryState::new())));
        RpcHandler::new(Network::Regtest, state)
    }

    #[test]
    fn get_info_reports_syncing() {
        let h = test_handler();
        let info = h.get_info();
        assert_eq!(info.protocol_version, 0);
        assert_eq!(info.network, "regtest");
        assert!(info.indexed_height.is_none());
        assert!(info.syncing);
    }

    #[test]
    fn chain_tip_reported() {
        let h = test_handler();
        *h.chain_tip_handle().write().unwrap() = Some(BlockRef {
            height: 10,
            hash: BlockHash([0u8; 32]),
        });
        let info = h.get_info();
        assert_eq!(info.chain_tip_height, Some(10));
    }
}
