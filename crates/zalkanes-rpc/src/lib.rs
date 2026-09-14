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
use std::time::Duration;

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
    /// Liveness of the indexing loop, when the node runs one. Absent means
    /// this handler serves a store without a live indexer attached.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub indexer: Option<IndexerInfo>,
}

/// Readiness verdict about the indexing loop, supplied by a [`LivenessProbe`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Liveness {
    /// No progress yet, still within the grace window.
    Starting,
    /// Advancing (or at the tip) recently.
    Healthy { caught_up: bool },
    /// Alive but no progress for longer than the stale window.
    Stalled {
        since: Duration,
        last_error: Option<String>,
    },
    /// The loop has exited on a fatal local error.
    Dead(String),
}

/// A point-in-time report from the indexing loop's bookkeeping.
#[derive(Debug, Clone)]
pub struct LivenessReport {
    pub liveness: Liveness,
    pub indexed_height: Option<BlockHeight>,
    pub tip_height: Option<BlockHeight>,
    pub last_progress_secs_ago: Option<u64>,
    pub last_tick_ok_secs_ago: Option<u64>,
    pub consecutive_failures: u32,
    pub total_failures: u64,
    pub last_error: Option<String>,
    pub dead_reason: Option<String>,
}

/// Source of indexer liveness for readiness and `zalkanes_getInfo`.
///
/// The node binary implements this over the indexing loop's health
/// bookkeeping; this crate deliberately does not depend on the indexer.
pub trait LivenessProbe: Send + Sync {
    fn report(&self) -> LivenessReport;
}

/// Liveness of the indexing loop, as exposed by `zalkanes_getInfo`.
///
/// `alive` says the loop has not died on a fatal local error; `advancing`
/// says it has made progress (or confirmed it is at the tip) within the
/// stale window. A process can be up with `alive: false` or
/// `advancing: false`; readiness reports both as unhealthy.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct IndexerInfo {
    /// `starting` | `healthy` | `syncing` | `stalled` | `dead`
    pub state: String,
    pub alive: bool,
    pub advancing: bool,
    pub indexed_height: Option<BlockHeight>,
    pub tip_height: Option<BlockHeight>,
    pub last_progress_secs_ago: Option<u64>,
    pub last_tick_ok_secs_ago: Option<u64>,
    pub consecutive_failures: u32,
    pub total_failures: u64,
    pub last_error: Option<String>,
    pub dead_reason: Option<String>,
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
    /// Indexer liveness, when a live indexing loop is attached.
    indexer: Option<Arc<dyn LivenessProbe>>,
}

impl RpcHandler {
    /// Build a handler from the SAME state handle the indexer uses.
    pub fn new(network: Network, state: SharedState) -> Self {
        Self {
            state,
            chain_tip: Arc::new(RwLock::new(None)),
            network,
            indexer: None,
        }
    }

    /// Attach the indexing loop's liveness probe. Readiness and
    /// `zalkanes_getInfo` then distinguish "process alive" from "indexer
    /// healthy and advancing".
    pub fn with_liveness_probe(mut self, probe: Arc<dyn LivenessProbe>) -> Self {
        self.indexer = Some(probe);
        self
    }

    /// Liveness of the attached indexer, assessed now.
    pub fn indexer_liveness(&self) -> Option<Liveness> {
        self.indexer.as_ref().map(|p| p.report().liveness)
    }

    fn indexer_info(&self) -> Option<IndexerInfo> {
        let r = self.indexer.as_ref()?.report();
        let (state, alive, advancing) = match &r.liveness {
            Liveness::Starting => ("starting", true, false),
            Liveness::Healthy { caught_up: true } => ("healthy", true, true),
            Liveness::Healthy { caught_up: false } => ("syncing", true, true),
            Liveness::Stalled { .. } => ("stalled", true, false),
            Liveness::Dead(_) => ("dead", false, false),
        };
        Some(IndexerInfo {
            state: state.to_string(),
            alive,
            advancing,
            indexed_height: r.indexed_height,
            tip_height: r.tip_height,
            last_progress_secs_ago: r.last_progress_secs_ago,
            last_tick_ok_secs_ago: r.last_tick_ok_secs_ago,
            consecutive_failures: r.consecutive_failures,
            total_failures: r.total_failures,
            last_error: r.last_error,
            dead_reason: r.dead_reason,
        })
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
            indexer: self.indexer_info(),
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
/// - `/health` → process alive (200 while up). Says nothing about indexing.
/// - `/ready`  → 200 caught up; 202 syncing or starting; 503 unhealthy.
///   With an indexer attached, a dead or stalled indexing loop is 503 even
///   though the process (and this endpoint) is alive; the body says why.
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
        "/health" => ("200 OK", "alive\n".to_string()),
        "/ready" => {
            let (state, detail) = readiness(&handler);
            let status = match state {
                HealthState::CaughtUp => "200 OK",
                HealthState::Syncing => "202 Accepted",
                HealthState::Starting => "200 OK",
                HealthState::Unhealthy => "503 Service Unavailable",
            };
            (status, format!("{detail}\n"))
        }
        _ => ("404 Not Found", "not_found\n".to_string()),
    };

    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    socket.write_all(response.as_bytes()).await?;
    socket.shutdown().await?;
    Ok(())
}

/// Readiness plus a one-line reason for the response body.
///
/// With an indexer attached, its liveness is authoritative: a dead or stalled
/// loop is `Unhealthy` regardless of how the stored heights look, because a
/// stale database behind a live RPC must not pass as ready.
pub fn readiness(handler: &RpcHandler) -> (HealthState, String) {
    match handler.indexer_liveness() {
        Some(Liveness::Dead(reason)) => (
            HealthState::Unhealthy,
            format!("unhealthy: indexer dead: {reason}"),
        ),
        Some(Liveness::Stalled { since, last_error }) => (
            HealthState::Unhealthy,
            format!(
                "unhealthy: indexer stalled for {}s{}",
                since.as_secs(),
                last_error
                    .map(|e| format!(" (last error: {e})"))
                    .unwrap_or_default()
            ),
        ),
        Some(Liveness::Starting) => (HealthState::Starting, "starting".to_string()),
        Some(Liveness::Healthy { caught_up: true }) => {
            (HealthState::CaughtUp, "caught_up".to_string())
        }
        Some(Liveness::Healthy { caught_up: false }) => {
            (HealthState::Syncing, "syncing".to_string())
        }
        None => {
            let info = handler.get_info();
            let state = match (info.indexed_height, info.chain_tip_height) {
                (Some(i), Some(t)) if i >= t => HealthState::CaughtUp,
                (Some(_), Some(_)) => HealthState::Syncing,
                (Some(_), None) => HealthState::Syncing,
                _ => HealthState::Starting,
            };
            let detail = match state {
                HealthState::CaughtUp => "caught_up",
                HealthState::Syncing => "syncing",
                HealthState::Starting => "starting",
                HealthState::Unhealthy => "unhealthy",
            };
            (state, detail.to_string())
        }
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

    struct StubProbe(LivenessReport);
    impl LivenessProbe for StubProbe {
        fn report(&self) -> LivenessReport {
            self.0.clone()
        }
    }

    fn report(liveness: Liveness, indexed: Option<u32>, tip: Option<u32>) -> LivenessReport {
        LivenessReport {
            dead_reason: match &liveness {
                Liveness::Dead(r) => Some(r.clone()),
                _ => None,
            },
            last_error: match &liveness {
                Liveness::Stalled { last_error, .. } => last_error.clone(),
                Liveness::Dead(r) => Some(r.clone()),
                _ => None,
            },
            liveness,
            indexed_height: indexed,
            tip_height: tip,
            last_progress_secs_ago: None,
            last_tick_ok_secs_ago: None,
            consecutive_failures: 0,
            total_failures: 0,
        }
    }

    fn with_probe(r: LivenessReport) -> RpcHandler {
        test_handler().with_liveness_probe(Arc::new(StubProbe(r)))
    }

    #[test]
    fn dead_indexer_is_unhealthy_even_though_the_process_is_up() {
        let h = with_probe(report(
            Liveness::Dead("rocksdb write batch failed".into()),
            Some(10),
            Some(10),
        ));
        let (state, detail) = readiness(&h);
        assert_eq!(state, HealthState::Unhealthy);
        assert!(detail.contains("indexer dead"), "{detail}");
        let info = h.get_info().indexer.unwrap();
        assert_eq!(info.state, "dead");
        assert!(!info.alive && !info.advancing);
        assert_eq!(
            info.dead_reason.as_deref(),
            Some("rocksdb write batch failed")
        );
    }

    #[test]
    fn stalled_indexer_is_unhealthy() {
        let mut r = report(
            Liveness::Stalled {
                since: Duration::from_secs(600),
                last_error: Some(
                    "RPC error -1: Provided index is greater than the current tip".into(),
                ),
            },
            Some(5),
            Some(9),
        );
        r.consecutive_failures = 7;
        let h = with_probe(r);
        let (state, detail) = readiness(&h);
        assert_eq!(state, HealthState::Unhealthy);
        assert!(detail.contains("stalled for 600s"), "{detail}");
        assert!(detail.contains("greater than the current tip"), "{detail}");
        let info = h.get_info().indexer.unwrap();
        assert_eq!(info.state, "stalled");
        assert!(info.alive, "a stalled loop is alive, just not advancing");
        assert!(!info.advancing);
        assert_eq!(info.consecutive_failures, 7);
    }

    #[test]
    fn advancing_indexer_is_ready() {
        let h = with_probe(report(
            Liveness::Healthy { caught_up: true },
            Some(10),
            Some(10),
        ));
        assert_eq!(readiness(&h).0, HealthState::CaughtUp);
        let info = h.get_info().indexer.unwrap();
        assert_eq!(info.state, "healthy");
        assert!(info.alive && info.advancing);

        let h = with_probe(report(
            Liveness::Healthy { caught_up: false },
            Some(4),
            Some(10),
        ));
        assert_eq!(readiness(&h).0, HealthState::Syncing);
        assert_eq!(h.get_info().indexer.unwrap().state, "syncing");
    }

    #[test]
    fn fresh_indexer_is_starting_within_the_grace_window() {
        let h = with_probe(report(Liveness::Starting, None, None));
        assert_eq!(readiness(&h).0, HealthState::Starting);
    }

    #[test]
    fn without_a_probe_readiness_falls_back_to_heights() {
        let h = test_handler();
        assert_eq!(readiness(&h).0, HealthState::Starting);
        assert!(h.get_info().indexer.is_none());
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
