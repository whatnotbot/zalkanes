//! Live indexing loop: pull canonical blocks from a [`ChainSource`], process
//! them, and persist state atomically.
//!
//! # Liveness contract
//!
//! The loop distinguishes two kinds of failure and treats them differently:
//!
//! - **Transient upstream failures** — any error talking to the chain source
//!   (tip fetch, `getblockhash`, raw block fetch), including the tip race
//!   where `getblockchaininfo` reports height `H` but `getblockhash(H)` answers
//!   "Provided index is greater than the current tip", and a block that fails
//!   to deserialize. These are logged, counted in [`IndexerHealth`], and
//!   retried on the next tick. **They never terminate the loop.**
//! - **Fatal local failures** — any error from the state store (`commit_block`,
//!   `rollback_to`, `save_executions`) or from executing a parsed block. These
//!   mean the local database can no longer be trusted to advance. The loop
//!   marks the indexer dead in [`IndexerHealth`] and returns the error; the
//!   caller decides what to do (the node exits non-zero so a supervisor
//!   restarts it rather than serving RPC over a dead indexer).
//!
//! Nothing here touches consensus: block processing is unchanged and still
//! goes through [`process_zcash_block`] / [`process_parsed_block`].

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use tracing::{error, info, warn};
use zalkanes_chain::ChainSource;
use zalkanes_core::types::{BlockHash, BlockHeight, BlockRef};
use zalkanes_state::{BlockCommit, StateStore};

use crate::{parse_block, process_parsed_block, process_zcash_block, IndexerConfig};

/// The state store shared between the indexer and the RPC server.
pub type SharedStore = Arc<RwLock<Box<dyn StateStore>>>;
/// The chain tip most recently observed, shared with the RPC server.
pub type TipHandle = Arc<RwLock<Option<BlockRef>>>;

/// Tunables of the loop itself (not consensus).
#[derive(Debug, Clone)]
pub struct SyncConfig {
    /// How often to poll the chain source for a new tip.
    pub poll_interval: Duration,
    /// After this long without progress the indexer is reported as stalled.
    pub stale_after: Duration,
}

impl Default for SyncConfig {
    fn default() -> Self {
        Self {
            poll_interval: Duration::from_secs(3),
            stale_after: Duration::from_secs(120),
        }
    }
}

// ── Health ───────────────────────────────────────────────────────────────────

/// A point-in-time copy of the indexer's liveness bookkeeping.
#[derive(Debug, Clone)]
pub struct IndexerStatus {
    pub started_at: Instant,
    /// Last tick that completed without an upstream failure.
    pub last_tick_ok: Option<Instant>,
    /// Last time the indexer advanced, or confirmed it is at the chain tip.
    pub last_progress: Option<Instant>,
    pub indexed_height: Option<BlockHeight>,
    pub tip_height: Option<BlockHeight>,
    pub consecutive_failures: u32,
    pub total_failures: u64,
    pub last_error: Option<String>,
    /// Set once and never cleared: the loop has terminated on a fatal error.
    pub dead: Option<String>,
}

/// What readiness should report, derived from [`IndexerStatus`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Liveness {
    /// No progress yet and still within the grace window.
    Starting,
    /// Advancing (or at the tip) recently; `caught_up` says which.
    Healthy { caught_up: bool },
    /// Alive, but no progress for longer than `stale_after` (upstream down,
    /// or a block that keeps failing to fetch/parse).
    Stalled {
        since: Duration,
        last_error: Option<String>,
    },
    /// The loop has exited on a fatal local error.
    Dead(String),
}

/// Shared liveness bookkeeping, updated by the loop and read by health probes.
#[derive(Debug)]
pub struct IndexerHealth {
    inner: Mutex<IndexerStatus>,
}

impl Default for IndexerHealth {
    fn default() -> Self {
        Self::new()
    }
}

impl IndexerHealth {
    pub fn new() -> Self {
        Self::new_at(Instant::now())
    }

    pub fn new_at(started_at: Instant) -> Self {
        Self {
            inner: Mutex::new(IndexerStatus {
                started_at,
                last_tick_ok: None,
                last_progress: None,
                indexed_height: None,
                tip_height: None,
                consecutive_failures: 0,
                total_failures: 0,
                last_error: None,
                dead: None,
            }),
        }
    }

    pub fn snapshot(&self) -> IndexerStatus {
        self.inner.lock().expect("health lock poisoned").clone()
    }

    /// A tick finished with no upstream failure. `advanced` is true when the
    /// indexed height moved or the indexer is at the tip.
    pub fn record_tick_ok(
        &self,
        indexed: Option<BlockHeight>,
        tip: BlockHeight,
        advanced: bool,
        now: Instant,
    ) {
        let mut s = self.inner.lock().expect("health lock poisoned");
        s.last_tick_ok = Some(now);
        if advanced {
            s.last_progress = Some(now);
        }
        s.indexed_height = indexed;
        s.tip_height = Some(tip);
        s.consecutive_failures = 0;
    }

    /// A transient upstream failure: counted and remembered, never fatal.
    pub fn record_failure(&self, err: &str, indexed: Option<BlockHeight>) {
        let mut s = self.inner.lock().expect("health lock poisoned");
        s.consecutive_failures = s.consecutive_failures.saturating_add(1);
        s.total_failures = s.total_failures.saturating_add(1);
        s.last_error = Some(err.to_string());
        s.indexed_height = indexed;
    }

    /// The loop has stopped for good.
    pub fn mark_dead(&self, reason: &str) {
        let mut s = self.inner.lock().expect("health lock poisoned");
        s.dead = Some(reason.to_string());
        s.last_error = Some(reason.to_string());
    }

    /// Derive readiness. A dead or stalled indexer is never healthy, no matter
    /// how alive the process is.
    pub fn assess(&self, now: Instant, stale_after: Duration) -> Liveness {
        let s = self.inner.lock().expect("health lock poisoned");
        if let Some(reason) = &s.dead {
            return Liveness::Dead(reason.clone());
        }
        match s.last_progress {
            Some(t) if now.saturating_duration_since(t) <= stale_after => {
                let caught_up =
                    matches!((s.indexed_height, s.tip_height), (Some(i), Some(t)) if i >= t);
                Liveness::Healthy { caught_up }
            }
            Some(t) => Liveness::Stalled {
                since: now.saturating_duration_since(t),
                last_error: s.last_error.clone(),
            },
            None if now.saturating_duration_since(s.started_at) <= stale_after => {
                Liveness::Starting
            }
            None => Liveness::Stalled {
                since: now.saturating_duration_since(s.started_at),
                last_error: s.last_error.clone(),
            },
        }
    }
}

// ── Error classification ─────────────────────────────────────────────────────

/// Why a tick did not complete.
#[derive(Debug)]
pub enum SyncError {
    /// Upstream (chain source) or block-decoding failure: retry next tick.
    Transient(anyhow::Error),
    /// Local state failure: the loop must stop.
    Fatal(anyhow::Error),
}

fn transient<T>(r: Result<T>, what: &str) -> std::result::Result<T, SyncError> {
    r.with_context(|| what.to_string())
        .map_err(SyncError::Transient)
}

fn fatal<T>(r: Result<T>, what: &str) -> std::result::Result<T, SyncError> {
    r.with_context(|| what.to_string())
        .map_err(SyncError::Fatal)
}

/// Run a blocking chain-source call off the async runtime. A join failure
/// (panic inside the call) is treated as transient: the source is retried.
async fn blocking<S, T, F>(source: &Arc<S>, f: F) -> Result<T>
where
    S: ChainSource + 'static,
    T: Send + 'static,
    F: FnOnce(&S) -> Result<T> + Send + 'static,
{
    let s = source.clone();
    tokio::task::spawn_blocking(move || f(&s))
        .await
        .map_err(|e| anyhow::anyhow!("chain source task failed: {e}"))?
}

/// What one tick achieved.
#[derive(Debug, Clone, Copy)]
pub struct TickOutcome {
    pub tip: BlockHeight,
    pub indexed_before: Option<BlockHeight>,
    pub indexed_after: Option<BlockHeight>,
}

// ── The loop ─────────────────────────────────────────────────────────────────

/// Run the indexing loop until `stop` is set (returns `Ok`) or a fatal local
/// error occurs (returns `Err`, after marking `health` dead).
///
/// Transient upstream errors are logged and retried on the next tick.
pub async fn run<S: ChainSource + 'static>(
    source: Arc<S>,
    store: SharedStore,
    tip_handle: TipHandle,
    config: IndexerConfig,
    sync: SyncConfig,
    health: Arc<IndexerHealth>,
    stop: Arc<AtomicBool>,
) -> Result<()> {
    let mut interval = tokio::time::interval(sync.poll_interval);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        interval.tick().await;
        if stop.load(Ordering::Relaxed) {
            return Ok(());
        }
        match tick(&source, &store, &tip_handle, &config, &stop).await {
            Ok(outcome) => {
                let advanced = outcome.indexed_after != outcome.indexed_before
                    || outcome.indexed_after.is_some_and(|h| h >= outcome.tip);
                health.record_tick_ok(outcome.indexed_after, outcome.tip, advanced, Instant::now());
            }
            Err(SyncError::Transient(e)) => {
                let indexed = store.read().expect("state lock poisoned").indexed_height();
                let msg = format!("{e:#}");
                health.record_failure(&msg, indexed);
                let failures = health.snapshot().consecutive_failures;
                warn!(
                    consecutive_failures = failures,
                    indexed = ?indexed,
                    error = %msg,
                    "transient upstream failure; retrying on the next tick"
                );
            }
            Err(SyncError::Fatal(e)) => {
                let msg = format!("{e:#}");
                health.mark_dead(&msg);
                error!(error = %msg, "fatal local error; indexing loop stopping");
                return Err(e);
            }
        }
    }
}

/// One iteration: refresh the tip, undo any reorg, fast-forward below
/// activation, then index every block up to the tip.
pub async fn tick<S: ChainSource + 'static>(
    source: &Arc<S>,
    store: &SharedStore,
    tip_handle: &TipHandle,
    config: &IndexerConfig,
    stop: &Arc<AtomicBool>,
) -> std::result::Result<TickOutcome, SyncError> {
    let indexed_before = store.read().expect("state lock poisoned").indexed_height();

    // 1. Refresh the chain tip. Upstream unreachable → transient.
    let tip = transient(blocking(source, |s| s.tip()).await, "fetching chain tip")?;
    *tip_handle.write().expect("tip lock poisoned") = Some(tip);

    // 2. Reorg detection: roll back until our stored hash matches canonical.
    handle_reorg(source, store).await?;

    // 3. Fast-forward below the activation height. Pre-activation blocks have
    //    no protocol effect (state is provably empty), so seek straight to
    //    `activation - 1` and record its canonical hash. Clamped to the tip so
    //    a future activation never asks for a block that does not exist yet;
    //    if the tip is ahead of what `getblockhash` can serve (a tip race),
    //    the fetch fails transiently and the next tick retries.
    if let Some(act) = config.activation_height() {
        if act > 1 {
            let target = (act - 1).min(tip.height);
            let below = store
                .read()
                .expect("state lock poisoned")
                .indexed_height()
                .is_none_or(|h| h < target);
            if below {
                let hash = transient(
                    blocking(source, move |s| s.block_hash(target)).await,
                    "fetching block hash for the fast-forward target",
                )?;
                let mut g = store.write().expect("state lock poisoned");
                fatal(
                    g.commit_block(BlockCommit {
                        height: target,
                        zcash_block_hash: hash,
                        deploys: Vec::new(),
                        upserts: Vec::new(),
                        deletes: Vec::new(),
                    })
                    .map(|_| ()),
                    "committing the fast-forward block",
                )?;
                info!(height = target, hash = %hash, "fast-forwarded below activation height");
            }
        }
    }

    // 4. Index every block up to the tip.
    loop {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        let indexed = store.read().expect("state lock poisoned").indexed_height();
        let next_height = indexed.map_or(1, |h| h + 1);
        if next_height > tip.height {
            break; // caught up
        }

        let hash = transient(
            blocking(source, move |s| s.block_hash(next_height)).await,
            "fetching canonical block hash",
        )?;

        // Below activation: no raw fetch, no deserialization; commit empty.
        let below_activation = config
            .activation_height()
            .is_some_and(|act| next_height < act);
        if below_activation {
            let mut g = store.write().expect("state lock poisoned");
            fatal(
                process_zcash_block(&mut **g, config, next_height, BlockHash(hash.0), &[])
                    .map(|_| ()),
                "committing a pre-activation block",
            )?;
            continue;
        }

        let raw = transient(
            blocking(source, move |s| s.raw_block(&hash)).await,
            "fetching raw block",
        )?;
        // A block that does not decode is retried, not fatal: the bytes came
        // from upstream and the local database is untouched.
        let parsed = transient(
            parse_block(&raw, next_height, BlockHash(hash.0), config.network),
            "parsing block",
        )?;
        let exec = {
            let mut g = store.write().expect("state lock poisoned");
            fatal(
                process_parsed_block(&mut **g, config, parsed),
                "processing block against local state",
            )?
        };
        info!(
            height = next_height,
            executions = exec.executions.len(),
            deployed = exec.deployed.len(),
            root = %exec.state_root_after,
            "indexed block"
        );
    }

    let indexed_after = store.read().expect("state lock poisoned").indexed_height();
    Ok(TickOutcome {
        tip: tip.height,
        indexed_before,
        indexed_after,
    })
}

/// Compare our stored hash at the indexed tip with the canonical chain and
/// roll back until they agree. Upstream failures are transient; a rollback
/// failure is fatal.
async fn handle_reorg<S: ChainSource + 'static>(
    source: &Arc<S>,
    store: &SharedStore,
) -> std::result::Result<(), SyncError> {
    loop {
        let (indexed_height, indexed_hash) = {
            let g = store.read().expect("state lock poisoned");
            (g.indexed_height(), g.indexed_block_hash())
        };
        let (Some(height), Some(our_hash)) = (indexed_height, indexed_hash) else {
            return Ok(());
        };

        let canonical = transient(
            blocking(source, move |s| s.block_hash(height)).await,
            "fetching canonical block hash for reorg check",
        )?;
        if canonical.0 == our_hash.0 {
            return Ok(());
        }

        warn!(
            height,
            ours = %hex::encode(our_hash.0),
            canonical = %hex::encode(canonical.0),
            "reorg detected; rolling back"
        );
        let mut g = store.write().expect("state lock poisoned");
        fatal(g.rollback_to(height - 1), "rolling back a reorged block")?;
    }
}
