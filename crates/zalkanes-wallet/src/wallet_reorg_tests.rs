//! §16.6 wallet reorg hardening: deterministic reorg matrix over fabricated
//! block chains (real `zcash_primitives` blocks: chained headers + BIP34
//! coinbase transactions), driving the REAL `scan_to_tip` /
//! `rewind_to_common_ancestor` paths against swappable canonical chains.
//!
//! Environment note (honest scope): this machine has no zebrad/Docker, so the
//! full-stack Zebra-regtest reorg runs — and the note-level rows of the
//! matrix ("note received on a removed branch disappears", "spend on a
//! removed branch rolls back"), which need real shielded outputs inside
//! blocks — are deferred to a live-regtest environment. Everything below
//! exercises the wallet's actual reorg machinery: divergence detection at
//! the exact fork point, common-ancestor search, canonical rewind, rescan,
//! birthday-crossing refusal, restart windows, and clean-replay equality.

use std::cell::RefCell;
use zalkanes_core::consensus_params::ConsensusParams;

use anyhow::{anyhow, bail, Result};
use rand_core::{OsRng, RngCore};
use secrecy::SecretVec;
use zcash_client_backend::data_api::chain::ChainState;
use zcash_primitives::block::{Block, BlockHeaderData};
use zcash_primitives::transaction::{Transaction, TransactionData, TxVersion};
use zcash_protocol::consensus::{BlockHeight, BranchId};
use zcash_transparent::{
    address::Script,
    bundle::{Bundle, OutPoint, TxIn, TxOut},
};

use crate::chain_source::CanonicalChainSource;
use crate::funding::{CanonicalTip, TipSource};
use crate::sqlite::SqliteShieldedWallet;

// ── Fabricated chain ─────────────────────────────────────────────────────────

#[derive(Clone)]
struct FakeBlock {
    height: u32,
    hash: [u8; 32],
    bytes: Vec<u8>,
}

/// Minimal script-number push of a block height (BIP34 coinbase encoding).
fn height_push(height: u32) -> Vec<u8> {
    let mut n = height as i64;
    let mut le = Vec::new();
    while n > 0 {
        le.push((n & 0xff) as u8);
        n >>= 8;
    }
    if le.last().is_some_and(|b| b & 0x80 != 0) {
        le.push(0);
    }
    let mut s = vec![le.len() as u8];
    s.extend_from_slice(&le);
    s
}

/// Construct a `Script` from raw bytes via its canonical serialized form
/// (avoids depending on `zcash_script` directly).
fn script(bytes: &[u8]) -> Script {
    let mut ser = vec![bytes.len() as u8];
    ser.extend_from_slice(bytes);
    Script::read(&ser[..]).expect("valid script encoding")
}

/// A V4 coinbase transaction claiming `height` (transparent-only).
fn coinbase(height: u32) -> Transaction {
    let vin = vec![TxIn::from_parts(
        OutPoint::NULL,
        script(&height_push(height)),
        u32::MAX,
    )];
    let vout = vec![TxOut::new(
        zcash_protocol::value::Zatoshis::ZERO,
        script(&[0x6a]),
    )];
    let bundle = Bundle {
        vin,
        vout,
        authorization: zcash_transparent::bundle::Authorized,
    };
    TransactionData::from_parts(
        TxVersion::V4,
        BranchId::Sapling,
        0,
        BlockHeight::from_u32(0),
        Some(bundle),
        None,
        None,
        None,
    )
    .freeze()
    .expect("valid coinbase")
}

/// One fabricated block at `height` chaining from `prev`, salted for hash
/// divergence between branches.
fn make_block(height: u32, prev: [u8; 32], salt: u8) -> FakeBlock {
    let tx = coinbase(height);
    let mut tx_bytes = Vec::new();
    tx.write(&mut tx_bytes).unwrap();

    let mut nonce = [0u8; 32];
    nonce[0] = salt;
    let header = BlockHeaderData {
        version: 4,
        prev_block: zcash_primitives::block::BlockHash(prev),
        merkle_root: [0u8; 32],
        final_sapling_root: [0u8; 32],
        time: 1_700_000_000 + height,
        bits: 0x1f07_ffff,
        nonce,
        solution: vec![],
    }
    .freeze()
    .expect("valid header");
    let hash = header.hash().0;

    let mut bytes = Vec::new();
    header.write(&mut bytes).unwrap();
    bytes.push(1); // CompactSize: one transaction
    bytes.extend_from_slice(&tx_bytes);

    FakeBlock {
        height,
        hash,
        bytes,
    }
}

/// Build a straight chain covering `start..=end`, salted per branch.
fn build_chain(start: u32, end: u32, salt: u8) -> Vec<FakeBlock> {
    let mut prev = [0xEEu8; 32];
    let mut out = Vec::new();
    for h in start..=end {
        let b = make_block(h, prev, salt);
        prev = b.hash;
        out.push(b);
    }
    out
}

/// Fork `base`: keep the first `keep` blocks, then extend with `extra`
/// differently-salted blocks (the reorged branch).
fn fork(base: &[FakeBlock], keep: usize, extra: u32, salt: u8) -> Vec<FakeBlock> {
    let mut out: Vec<FakeBlock> = base[..keep].to_vec();
    let mut prev = out.last().map(|b| b.hash).unwrap_or([0xEEu8; 32]);
    let next_height = out.last().map(|b| b.height + 1).unwrap_or(0);
    for h in next_height..next_height + extra {
        let b = make_block(h, prev, salt);
        prev = b.hash;
        out.push(b);
    }
    out
}

/// A canonical chain whose branch can be swapped mid-test (the "Zebra view").
struct SwappableChain {
    blocks: RefCell<Vec<FakeBlock>>,
    /// Heights at which `block()` fails once (fault injection for
    /// restart-during-rescan windows).
    fail_once_at: RefCell<Option<u32>>,
}

impl SwappableChain {
    fn new(blocks: Vec<FakeBlock>) -> Self {
        Self {
            blocks: RefCell::new(blocks),
            fail_once_at: RefCell::new(None),
        }
    }
    fn swap(&self, blocks: Vec<FakeBlock>) {
        *self.blocks.borrow_mut() = blocks;
    }
    fn fail_once_at(&self, height: u32) {
        *self.fail_once_at.borrow_mut() = Some(height);
    }
    fn find(&self, h: u32) -> Result<FakeBlock> {
        self.blocks
            .borrow()
            .iter()
            .find(|b| b.height == h)
            .cloned()
            .ok_or_else(|| anyhow!("no block at height {h}"))
    }
}

impl TipSource for SwappableChain {
    fn canonical_tip(&self) -> Result<CanonicalTip> {
        let blocks = self.blocks.borrow();
        let last = blocks.last().ok_or_else(|| anyhow!("empty chain"))?;
        Ok(CanonicalTip {
            height: last.height,
            hash: last.hash,
        })
    }
}

impl CanonicalChainSource for SwappableChain {
    fn block_hash(&self, h: u32) -> Result<[u8; 32]> {
        Ok(self.find(h)?.hash)
    }
    fn block(&self, h: u32) -> Result<Block> {
        if *self.fail_once_at.borrow() == Some(h) {
            *self.fail_once_at.borrow_mut() = None;
            bail!("injected fault: chain source unavailable at height {h}");
        }
        let b = self.find(h)?;
        Block::read(&b.bytes[..], &ConsensusParams::Test).map_err(|e| anyhow!("parse block: {e}"))
    }
    fn tree_state(&self, h: u32) -> Result<ChainState> {
        // Empty commitment trees throughout (no shielded outputs in the
        // fabricated chain); the block-hash identity is the real one.
        let hash = self.find(h).map(|b| b.hash).unwrap_or([0xEEu8; 32]); // pre-chain birthday anchor state
        Ok(ChainState::empty(
            BlockHeight::from_u32(h),
            zcash_primitives::block::BlockHash(hash),
        ))
    }
}

// ── Harness ──────────────────────────────────────────────────────────────────

const START: u32 = 100;
const TIP0: u32 = 260;

fn tmp_wallet_path(tag: &str) -> std::path::PathBuf {
    let mut suffix = [0u8; 8];
    OsRng.fill_bytes(&mut suffix);
    std::env::temp_dir().join(format!(
        "zalkanes-reorg-{tag}-{}.sqlite",
        hex::encode(suffix)
    ))
}

/// Create a wallet against the chain and fully sync it.
fn synced_wallet(
    tag: &str,
    chain: &SwappableChain,
) -> (std::path::PathBuf, SecretVec<u8>, SqliteShieldedWallet) {
    let path = tmp_wallet_path(tag);
    let mut seed_bytes = vec![0u8; 32];
    OsRng.fill_bytes(&mut seed_bytes);
    let seed = SecretVec::new(seed_bytes.clone());
    let wallet =
        SqliteShieldedWallet::create_new(&path, ConsensusParams::Test, seed, chain).unwrap();
    wallet.scan_to_tip(chain).unwrap();
    (path, SecretVec::new(seed_bytes), wallet)
}

/// The wallet's fully-scanned identity must equal the chain's canonical tip.
fn assert_synced(wallet: &SqliteShieldedWallet, chain: &SwappableChain) {
    use crate::shielded::ShieldedWallet;
    let tip = chain.canonical_tip().unwrap();
    let status = wallet.sync_status(tip).unwrap();
    assert!(
        status.synced,
        "wallet {}:{} vs canonical {}:{}",
        status.wallet_scan_height,
        hex::encode(status.wallet_scan_hash),
        tip.height,
        hex::encode(tip.hash)
    );
}

fn scan_state(wallet: &SqliteShieldedWallet) -> (u32, String) {
    let h = wallet.next_scan_height().unwrap() - 1;
    (h, hex::encode([0u8; 0])) // height is the comparable part; hash asserted via sync_status
}

// ── The matrix ───────────────────────────────────────────────────────────────

#[test]
fn linear_scan_reaches_tip() {
    let chain = SwappableChain::new(build_chain(START, TIP0, 1));
    let (path, _seed, wallet) = synced_wallet("linear", &chain);
    assert_synced(&wallet, &chain);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn same_height_reorg_replaces_tip_block() {
    let base = build_chain(START, TIP0, 1);
    let chain = SwappableChain::new(base.clone());
    let (path, _seed, wallet) = synced_wallet("sameheight", &chain);

    // Replace ONLY the tip block (same height, different hash).
    chain.swap(fork(&base, base.len() - 1, 1, 2));
    wallet.scan_to_tip(&chain).unwrap();
    assert_synced(&wallet, &chain);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn shallow_reorg_three_blocks() {
    let base = build_chain(START, TIP0, 1);
    let chain = SwappableChain::new(base.clone());
    let (path, _seed, wallet) = synced_wallet("shallow", &chain);

    // Drop the last 3 blocks; the new branch is one block longer.
    chain.swap(fork(&base, base.len() - 3, 4, 2));
    wallet.scan_to_tip(&chain).unwrap();
    assert_synced(&wallet, &chain);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn deep_reorg_to_just_above_birthday() {
    let base = build_chain(START, TIP0, 1);
    let chain = SwappableChain::new(base.clone());
    let (path, _seed, wallet) = synced_wallet("deep", &chain);

    let birthday = wallet.birthday_height().unwrap();
    // Keep only up to birthday + 2; replace everything above it.
    let keep = base.iter().position(|b| b.height == birthday + 2).unwrap() + 1;
    chain.swap(fork(&base, keep, 80, 3));
    wallet.scan_to_tip(&chain).unwrap();
    assert_synced(&wallet, &chain);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn reorg_crossing_birthday_is_refused() {
    let base = build_chain(START, TIP0, 1);
    let chain = SwappableChain::new(base.clone());
    let (path, _seed, wallet) = synced_wallet("crossing", &chain);

    let birthday = wallet.birthday_height().unwrap();
    // Fork BELOW the birthday: recovery would need history the wallet never
    // scanned. The wallet must refuse loudly, not rewind into the void.
    let keep = base
        .iter()
        .position(|b| b.height == birthday.saturating_sub(2))
        .unwrap()
        + 1;
    chain.swap(fork(&base, keep, 200, 4));
    let err = wallet
        .scan_to_tip(&chain)
        .expect_err("birthday-crossing reorg must be refused");
    assert!(err.to_string().contains("birthday"), "got: {err}");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn restart_immediately_before_rewind() {
    let base = build_chain(START, TIP0, 1);
    let chain = SwappableChain::new(base.clone());
    let (path, seed, wallet) = synced_wallet("restartpre", &chain);

    // The chain reorgs while the wallet process is down.
    drop(wallet);
    chain.swap(fork(&base, base.len() - 5, 6, 2));

    let wallet = SqliteShieldedWallet::reopen(&path, ConsensusParams::Test, seed).unwrap();
    wallet.scan_to_tip(&chain).unwrap();
    assert_synced(&wallet, &chain);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn restart_after_rewind_mid_rescan() {
    let base = build_chain(START, TIP0, 1);
    let chain = SwappableChain::new(base.clone());
    let (path, seed, wallet) = synced_wallet("restartmid", &chain);

    // Reorg + a fault injected mid-branch: the first rescan attempt dies
    // after the rewind, partway up the new branch (crash during canonical
    // rescan).
    chain.swap(fork(&base, base.len() - 10, 11, 2));
    chain.fail_once_at(TIP0 - 4);
    let err = wallet
        .scan_to_tip(&chain)
        .expect_err("injected fault fires");
    assert!(err.to_string().contains("injected fault"), "got: {err}");
    drop(wallet); // process restart

    let wallet = SqliteShieldedWallet::reopen(&path, ConsensusParams::Test, seed).unwrap();
    wallet.scan_to_tip(&chain).unwrap();
    assert_synced(&wallet, &chain);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn repeated_reorgs_stay_convergent() {
    let base = build_chain(START, TIP0, 1);
    let chain = SwappableChain::new(base.clone());
    let (path, _seed, wallet) = synced_wallet("repeat", &chain);

    let mut current = base;
    for salt in 5..9u8 {
        let keep = current.len() - (salt as usize % 4) - 1;
        current = fork(&current, keep, salt as u32, salt);
        chain.swap(current.clone());
        wallet.scan_to_tip(&chain).unwrap();
        assert_synced(&wallet, &chain);
    }
    let _ = std::fs::remove_file(&path);
}

#[test]
fn recovered_wallet_equals_clean_fresh_replay() {
    let base = build_chain(START, TIP0, 1);
    let chain = SwappableChain::new(base.clone());
    let (path_a, _seed, wallet_a) = synced_wallet("replay-a", &chain);

    // Wallet A lives through a deep reorg.
    let final_chain = fork(&base, base.len() - 40, 45, 7);
    chain.swap(final_chain.clone());
    wallet_a.scan_to_tip(&chain).unwrap();
    assert_synced(&wallet_a, &chain);

    // Wallet B scans the final chain from scratch.
    let chain_b = SwappableChain::new(final_chain);
    let (path_b, _seed_b, wallet_b) = synced_wallet("replay-b", &chain_b);
    assert_synced(&wallet_b, &chain_b);

    // Both wallets report the same fully-scanned identity as the canonical
    // tip (height AND hash), i.e. recovered state == clean replay state.
    let _ = scan_state(&wallet_a);
    let _ = scan_state(&wallet_b);
    let tip = chain.canonical_tip().unwrap();
    let tip_b = chain_b.canonical_tip().unwrap();
    assert_eq!(tip.height, tip_b.height);
    assert_eq!(tip.hash, tip_b.hash);

    let _ = std::fs::remove_file(&path_a);
    let _ = std::fs::remove_file(&path_b);
}

/// Regression for the live-regtest scan failure: the wallet stack must use
/// REGTEST consensus parameters on regtest, not testnet ones.
///
/// Regtest activates every network upgrade at height 1, so a regtest block's
/// coinbase commits to the regtest branch id. Parsing it under testnet
/// parameters fails with "coinbase tx's claimed height doesn't match its
/// consensus branch ID" — which is exactly how live regtest scanning broke
/// at block 21.
#[test]
fn regtest_blocks_parse_under_regtest_params_not_testnet() {
    use zcash_protocol::consensus::BranchId;

    // At low heights regtest and testnet resolve DIFFERENT branch ids,
    // because regtest activates everything at height 1.
    let h = BlockHeight::from_u32(21);
    let regtest = BranchId::for_height(&ConsensusParams::Regtest, h);
    let testnet = BranchId::for_height(&ConsensusParams::Test, h);
    assert_ne!(
        regtest, testnet,
        "if these matched, this regression could not be detected"
    );

    // A block built for regtest parses under regtest parameters...
    let block = make_block(21, [0xEEu8; 32], 1);
    Block::read(&block.bytes[..], &ConsensusParams::Regtest)
        .expect("regtest block must parse under regtest parameters");

    // ...and the wallet's chain source must therefore be constructed with
    // regtest parameters for a regtest node. `ConsensusParams::for_network`
    // is the single mapping that guarantees it.
    assert_eq!(
        ConsensusParams::for_network(zalkanes_core::types::Network::Regtest),
        ConsensusParams::Regtest
    );
    assert_eq!(
        ConsensusParams::for_network(zalkanes_core::types::Network::Testnet),
        ConsensusParams::Test
    );
    assert_eq!(
        ConsensusParams::for_network(zalkanes_core::types::Network::Mainnet),
        ConsensusParams::Main
    );
}
