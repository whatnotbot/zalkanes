//! Liveness regressions for the indexing loop (`zalkanes_indexer::sync`).
//!
//! Reproduces the live failure seen on the public-testnet nodes: Zebra's
//! `getblockchaininfo` reported tip `H` while `getblockhash(H)` answered
//! "Provided index is greater than the current tip", and the below-activation
//! fast-forward propagated that transient error out of the loop, leaving the
//! RPC process alive over a dead indexer.
//!
//! The chain source is a scripted mock; the state store is the real in-memory
//! store; the loop is the real `sync::run`.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use anyhow::{bail, Result};
use zalkanes_chain::ChainSource;
use zalkanes_core::types::{BlockHash, BlockHeight, BlockRef, Network};
use zalkanes_indexer::sync::{self, IndexerHealth, Liveness, SharedStore, SyncConfig, TipHandle};
use zalkanes_indexer::IndexerConfig;
use zalkanes_state::{MemoryState, StateStore};

/// The exact upstream error text observed on Railway.
const TIP_RACE: &str = "RPC error -1: Provided index is greater than the current tip";

/// A scripted chain: `blocks[h]` is the canonical hash at height `h`;
/// `tip_height` is what `getblockchaininfo` claims (it may run AHEAD of the
/// blocks `getblockhash` can serve, which is the race under test).
/// `tip_faults` / `hash_faults` are queues of errors returned by the next
/// calls, to simulate an unreachable or flaky upstream.
struct ScriptedChain {
    blocks: Mutex<Vec<BlockHash>>,
    tip_height: Mutex<BlockHeight>,
    tip_faults: Mutex<VecDeque<String>>,
    hash_faults: Mutex<VecDeque<String>>,
    tip_calls: AtomicU64,
    hash_calls: AtomicU64,
}

fn hash_at(h: BlockHeight) -> BlockHash {
    let mut b = [0u8; 32];
    b[..4].copy_from_slice(&h.to_be_bytes());
    b[31] = 0xAB;
    BlockHash(b)
}

impl ScriptedChain {
    fn with_blocks(n: BlockHeight) -> Arc<Self> {
        Arc::new(Self {
            blocks: Mutex::new((0..=n).map(hash_at).collect()),
            tip_height: Mutex::new(n),
            tip_faults: Mutex::new(VecDeque::new()),
            hash_faults: Mutex::new(VecDeque::new()),
            tip_calls: AtomicU64::new(0),
            hash_calls: AtomicU64::new(0),
        })
    }
    /// Claim a tip height (without necessarily having the block).
    fn claim_tip(&self, h: BlockHeight) {
        *self.tip_height.lock().unwrap() = h;
    }
    /// Mine blocks up to and including `h`, and claim that tip.
    fn extend_to(&self, h: BlockHeight) {
        let mut b = self.blocks.lock().unwrap();
        while (b.len() as BlockHeight) <= h {
            let next = b.len() as BlockHeight;
            b.push(hash_at(next));
        }
        drop(b);
        self.claim_tip(h);
    }
    fn fail_tip_next(&self, n: usize, msg: &str) {
        let mut q = self.tip_faults.lock().unwrap();
        for _ in 0..n {
            q.push_back(msg.to_string());
        }
    }
    fn fail_hash_next(&self, n: usize, msg: &str) {
        let mut q = self.hash_faults.lock().unwrap();
        for _ in 0..n {
            q.push_back(msg.to_string());
        }
    }
}

impl ChainSource for ScriptedChain {
    fn network(&self) -> Network {
        Network::Regtest
    }
    fn tip(&self) -> Result<BlockRef> {
        self.tip_calls.fetch_add(1, Ordering::Relaxed);
        if let Some(msg) = self.tip_faults.lock().unwrap().pop_front() {
            bail!("{msg}");
        }
        let h = *self.tip_height.lock().unwrap();
        // Like Zebra, the tip's hash is whatever we would serve for it (the
        // race only concerns getblockhash for heights not yet served).
        let hash = self
            .blocks
            .lock()
            .unwrap()
            .get(h as usize)
            .copied()
            .unwrap_or(hash_at(h));
        Ok(BlockRef { height: h, hash })
    }
    fn block_hash(&self, height: BlockHeight) -> Result<BlockHash> {
        self.hash_calls.fetch_add(1, Ordering::Relaxed);
        if let Some(msg) = self.hash_faults.lock().unwrap().pop_front() {
            bail!("{msg}");
        }
        match self.blocks.lock().unwrap().get(height as usize) {
            Some(h) => Ok(*h),
            None => bail!("{TIP_RACE}"),
        }
    }
    fn raw_block(&self, _hash: &BlockHash) -> Result<Vec<u8>> {
        // Every test runs below the (overridden) activation height, where the
        // loop never fetches raw blocks. Reaching here would be a bug.
        bail!("raw_block must not be called below activation in these tests")
    }
}

struct Harness {
    chain: Arc<ScriptedChain>,
    store: SharedStore,
    tip: TipHandle,
    health: Arc<IndexerHealth>,
    stop: Arc<AtomicBool>,
    task: tokio::task::JoinHandle<Result<()>>,
}

const STALE_AFTER: Duration = Duration::from_millis(400);

/// Start the real loop against `chain` with the activation height moved to
/// `activation` (regtest override), polling every 10 ms.
fn start(chain: Arc<ScriptedChain>, activation: BlockHeight) -> Harness {
    let store: SharedStore = Arc::new(RwLock::new(Box::new(MemoryState::new())));
    start_with_store(chain, activation, store)
}

fn start_with_store(
    chain: Arc<ScriptedChain>,
    activation: BlockHeight,
    store: SharedStore,
) -> Harness {
    let tip: TipHandle = Arc::new(RwLock::new(None));
    let health = Arc::new(IndexerHealth::new());
    let stop = Arc::new(AtomicBool::new(false));
    let config = IndexerConfig {
        network: Network::Regtest,
        data_dir: std::path::PathBuf::from("/nonexistent"),
        regtest_activation_override: Some(activation),
    };
    let sync_cfg = SyncConfig {
        poll_interval: Duration::from_millis(10),
        stale_after: STALE_AFTER,
    };
    let task = tokio::spawn(sync::run(
        chain.clone(),
        store.clone(),
        tip.clone(),
        config,
        sync_cfg,
        health.clone(),
        stop.clone(),
    ));
    Harness {
        chain,
        store,
        tip,
        health,
        stop,
        task,
    }
}

impl Harness {
    fn indexed(&self) -> Option<BlockHeight> {
        self.store.read().unwrap().indexed_height()
    }
    fn indexed_hash(&self) -> Option<BlockHash> {
        self.store.read().unwrap().indexed_block_hash()
    }
    async fn wait_until(&self, what: &str, mut f: impl FnMut(&Self) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !f(self) {
            assert!(
                !self.task.is_finished(),
                "indexing task exited while waiting for: {what}"
            );
            assert!(Instant::now() < deadline, "timed out waiting for: {what}");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }
    async fn stop(self) -> Result<()> {
        self.stop.store(true, Ordering::Relaxed);
        self.task.await.expect("indexing task panicked")
    }
}

// ── 5. the exact tip race ────────────────────────────────────────────────────

/// (a) tip says H, (b) getblockhash(H) says "index greater than current tip",
/// (c) the node survives, (d) a later iteration sees the canonical tip,
/// (e) indexing continues normally.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tip_race_during_fast_forward_does_not_kill_the_loop() {
    // Canonical chain has blocks 0..=49; Zebra claims tip 50 before it can
    // serve getblockhash(50). Activation at 1000 puts every height below it,
    // so the fast-forward targets min(999, tip) = 50 — the failing call.
    let chain = ScriptedChain::with_blocks(49);
    chain.claim_tip(50);
    let h = start(chain.clone(), 1_000);

    // (b)+(c): the fault is observed, counted, and the task is still running.
    h.wait_until("the tip race to be recorded as a transient failure", |h| {
        h.health.snapshot().consecutive_failures >= 3
    })
    .await;
    let s = h.health.snapshot();
    assert!(!h.task.is_finished(), "loop must survive the tip race");
    assert!(s.dead.is_none());
    assert!(
        s.last_error
            .as_deref()
            .unwrap_or("")
            .contains("greater than the current tip"),
        "last_error should carry the upstream reason: {:?}",
        s.last_error
    );
    assert_eq!(
        h.indexed(),
        None,
        "nothing must be committed on a failed fast-forward"
    );

    // (d): the canonical chain catches up with the claimed tip.
    h.chain.extend_to(50);
    h.wait_until("the fast-forward to land at height 50", |h| {
        h.indexed() == Some(50)
    })
    .await;
    assert_eq!(
        h.indexed_hash(),
        Some(hash_at(50)),
        "recorded hash must be canonical"
    );
    h.wait_until("failure counter to reset after a good tick", |h| {
        h.health.snapshot().consecutive_failures == 0
    })
    .await;
    assert_eq!(
        h.health.assess(Instant::now(), STALE_AFTER),
        Liveness::Healthy { caught_up: true }
    );

    // (e): indexing continues block by block as the chain grows.
    h.chain.extend_to(57);
    h.wait_until("indexing to continue to height 57", |h| {
        h.indexed() == Some(57)
    })
    .await;
    assert_eq!(h.indexed_hash(), Some(hash_at(57)));
    assert_eq!(h.tip.read().unwrap().map(|t| t.height), Some(57));
    let s = h.health.snapshot();
    assert!(s.dead.is_none());
    assert!(
        s.total_failures >= 3,
        "the earlier faults stay in the total"
    );
    assert_eq!(s.consecutive_failures, 0);

    h.stop().await.expect("loop stops cleanly");
}

/// The same race, but after the fast-forward: the chain is above activation
/// height bookkeeping-wise still below (override), the indexer is caught up,
/// and Zebra claims a tip one ahead of what it serves. The catch-up loop's
/// getblockhash fails transiently; the loop keeps the previous progress.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tip_race_during_catch_up_keeps_previous_progress() {
    let chain = ScriptedChain::with_blocks(20);
    let h = start(chain.clone(), 1_000);
    h.wait_until("initial catch-up to 20", |h| h.indexed() == Some(20))
        .await;

    h.chain.claim_tip(21); // block 21 not yet servable
    h.wait_until("transient failure at the claimed tip", |h| {
        h.health.snapshot().consecutive_failures >= 2
    })
    .await;
    assert_eq!(h.indexed(), Some(20), "progress must not regress");
    assert!(!h.task.is_finished());

    h.chain.extend_to(23);
    h.wait_until("catch-up to 23", |h| h.indexed() == Some(23))
        .await;
    assert_eq!(h.indexed_hash(), Some(hash_at(23)));
    h.stop().await.unwrap();
}

// ── 6. consecutive transient failures ────────────────────────────────────────

/// Many consecutive upstream failures (tip fetch AND getblockhash, mixed)
/// never kill the task; once upstream recovers, indexing resumes.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn many_consecutive_transient_failures_do_not_kill_the_task() {
    let chain = ScriptedChain::with_blocks(30);
    let h = start(chain.clone(), 1_000);
    h.wait_until("initial catch-up to 30", |h| h.indexed() == Some(30))
        .await;

    // 40 failed tip fetches, then 20 failed getblockhash calls in a row.
    h.chain.fail_tip_next(
        40,
        "RPC call to getblockchaininfo failed: connection refused",
    );
    h.chain
        .fail_hash_next(20, "RPC error -28: Loading block index...");
    h.wait_until("all 60 faults to be consumed", |h| {
        h.health.snapshot().total_failures >= 60
    })
    .await;
    assert!(
        !h.task.is_finished(),
        "60 consecutive transient failures must not kill the loop"
    );
    let s = h.health.snapshot();
    assert!(s.dead.is_none());
    assert!(s.consecutive_failures >= 60);
    assert_eq!(h.indexed(), Some(30), "state untouched throughout");

    // Upstream recovers and the chain has grown meanwhile.
    h.chain.extend_to(35);
    h.wait_until("indexing to resume to 35", |h| h.indexed() == Some(35))
        .await;
    h.wait_until("failure streak to reset", |h| {
        h.health.snapshot().consecutive_failures == 0
    })
    .await;
    assert!(matches!(
        h.health.assess(Instant::now(), STALE_AFTER),
        Liveness::Healthy { caught_up: true }
    ));
    h.stop().await.unwrap();
}

/// While upstream is down long enough, readiness must report the loop as
/// stalled (alive but not advancing), and recover once it advances again.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn prolonged_upstream_outage_is_reported_as_stalled_then_recovers() {
    let chain = ScriptedChain::with_blocks(10);
    let h = start(chain.clone(), 1_000);
    h.wait_until("initial catch-up", |h| h.indexed() == Some(10))
        .await;

    h.chain.fail_tip_next(10_000, "connection refused");
    tokio::time::sleep(STALE_AFTER + Duration::from_millis(100)).await;
    assert!(!h.task.is_finished());
    assert!(
        matches!(
            h.health.assess(Instant::now(), STALE_AFTER),
            Liveness::Stalled { .. }
        ),
        "no progress for longer than stale_after must read as stalled"
    );

    // Drain the fault queue so the next tick succeeds.
    h.chain.tip_faults.lock().unwrap().clear();
    h.chain.extend_to(12);
    h.wait_until("recovery to 12", |h| h.indexed() == Some(12))
        .await;
    assert!(matches!(
        h.health.assess(Instant::now(), STALE_AFTER),
        Liveness::Healthy { caught_up: true }
    ));
    h.stop().await.unwrap();
}

// ── 3. fatal local errors are NOT swallowed ──────────────────────────────────

/// A store whose commits fail permanently (disk full, corrupt db, …).
struct BrokenStore {
    inner: MemoryState,
    fail_commits: Arc<AtomicBool>,
}

impl StateStore for BrokenStore {
    fn has_contract(&self, id: &zalkanes_core::types::ContractId) -> bool {
        self.inner.has_contract(id)
    }
    fn get_contract(
        &self,
        id: &zalkanes_core::types::ContractId,
    ) -> Option<(zalkanes_core::types::CodeHash, Vec<u8>)> {
        self.inner.get_contract(id)
    }
    fn storage_get(
        &self,
        contract: &zalkanes_core::types::ContractId,
        key: &[u8],
    ) -> Option<Vec<u8>> {
        self.inner.storage_get(contract, key)
    }
    fn compute_root(&self) -> zalkanes_core::types::StateRoot {
        self.inner.compute_root()
    }
    fn for_each_contract(&self, f: &mut zalkanes_state::ContractVisitor<'_>) {
        self.inner.for_each_contract(f)
    }
    fn for_each_storage(&self, f: &mut zalkanes_state::StorageVisitor<'_>) {
        self.inner.for_each_storage(f)
    }
    fn indexed_height(&self) -> Option<BlockHeight> {
        self.inner.indexed_height()
    }
    fn indexed_block_hash(&self) -> Option<BlockHash> {
        self.inner.indexed_block_hash()
    }
    fn persisted_state_root(&self) -> Option<zalkanes_core::types::StateRoot> {
        self.inner.persisted_state_root()
    }
    fn height_history(&self) -> Vec<zalkanes_state::HeightRecord> {
        self.inner.height_history()
    }
    fn commit_block(
        &mut self,
        commit: zalkanes_state::BlockCommit,
    ) -> Result<zalkanes_core::types::StateRoot> {
        if self.fail_commits.load(Ordering::Relaxed) {
            bail!("rocksdb write batch failed: IO error: No space left on device");
        }
        self.inner.commit_block(commit)
    }
    fn rollback_to(&mut self, target_height: BlockHeight) -> Result<()> {
        self.inner.rollback_to(target_height)
    }
    fn save_executions(&mut self, executions: &[zalkanes_core::types::Execution]) -> Result<()> {
        self.inner.save_executions(executions)
    }
    fn execution(
        &self,
        txid: &zalkanes_core::types::TxId,
    ) -> Option<zalkanes_core::types::Execution> {
        self.inner.execution(txid)
    }
    fn executions_at_height(&self, height: BlockHeight) -> Vec<zalkanes_core::types::Execution> {
        self.inner.executions_at_height(height)
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fatal_local_state_error_stops_the_loop_and_marks_it_dead() {
    let chain = ScriptedChain::with_blocks(10);
    let fail = Arc::new(AtomicBool::new(false));
    let store: SharedStore = Arc::new(RwLock::new(Box::new(BrokenStore {
        inner: MemoryState::new(),
        fail_commits: fail.clone(),
    })));
    let h = start_with_store(chain.clone(), 1_000, store);
    h.wait_until("initial catch-up", |h| h.indexed() == Some(10))
        .await;

    // The database starts refusing writes; the next block must not be
    // silently skipped or retried forever — the loop reports and stops.
    fail.store(true, Ordering::Relaxed);
    h.chain.extend_to(11);
    let deadline = Instant::now() + Duration::from_secs(5);
    while !h.task.is_finished() {
        assert!(
            Instant::now() < deadline,
            "loop should have stopped on a fatal state error"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let Harness {
        task,
        health,
        store,
        ..
    } = h;
    let err = task
        .await
        .unwrap()
        .expect_err("run must return the fatal error");
    assert!(
        format!("{err:#}").contains("No space left on device"),
        "{err:#}"
    );
    let s = health.snapshot();
    assert!(s
        .dead
        .as_deref()
        .unwrap_or("")
        .contains("No space left on device"));
    assert!(matches!(
        health.assess(Instant::now(), STALE_AFTER),
        Liveness::Dead(_)
    ));
    assert_eq!(
        store.read().unwrap().indexed_height(),
        Some(10),
        "the failed commit must not have advanced the height"
    );
}
