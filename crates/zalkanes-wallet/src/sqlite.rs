//! `SqliteShieldedWallet`: the `zcash_client_sqlite`-backed implementation of
//! [`ShieldedWallet`].
//!
//! Uses the canonical wallet database as the source of truth for accounts,
//! notes, spent/unspent status, scan height, witnesses/anchors, transactions,
//! and balances. Note reservations use the wallet's own durable output-locking
//! (`OutputLockStore::lock_outputs` / `unlock_output`), which is persisted in
//! the wallet SQLite database and reconciles across process restarts.

#![forbid(unsafe_code)]

use std::cell::{Cell, RefCell};

use anyhow::{anyhow, bail, Result};
use orchard::{
    keys::{FullViewingKey, SpendAuthorizingKey},
    Anchor, ValuePool,
};
use rand_core::OsRng;
use secrecy::{ExposeSecret, SecretVec};
use sha2::{Digest, Sha256};
use zcash_client_backend::{
    data_api::{
        chain::ChainState,
        locking::{LockOwner, OutputLockStore},
        wallet::{
            input_selection::{LockFilter, LockedInputPolicy},
            ConfirmationsPolicy, TargetHeight,
        },
        AccountBirthday, BlockMetadata, InputSource, TargetValue, WalletCommitmentTrees,
        WalletRead, WalletWrite,
    },
    scanning::{self, Nullifiers, ScanningKeys},
    wallet::{OutputRef, ReceivedNote},
};
use zcash_client_sqlite::{util::SystemClock, AccountUuid, ReceivedNoteId, WalletDb};
use zcash_keys::{
    address::UnifiedAddress,
    keys::{UnifiedAddressRequest, UnifiedSpendingKey},
};
use zcash_protocol::{
    consensus::{BlockHeight, Network},
    ShieldedPool,
};
use zip32::AccountId;

use super::{ShieldedSelection, ShieldedSpend, ShieldedWallet, SyncStatus};

/// The concrete wallet database type used here.
type Db = WalletDb<rusqlite::Connection, Network, SystemClock, OsRng>;

/// A shielded wallet backed by `zcash_client_sqlite`.
pub struct SqliteShieldedWallet {
    db: RefCell<Db>,
    account_id: AccountUuid,
    orchard_fvk: FullViewingKey,
    orchard_ask: SpendAuthorizingKey,
    zebra_tip: Cell<u32>,
    zebra_tip_hash: Cell<[u8; 32]>,
}

impl SqliteShieldedWallet {
    /// Shared open + key-derivation helper. Opens the DB, derives the Orchard
    /// spending keys from `seed` (held in memory only), and initializes schema.
    fn open_db_and_derive(
        path: &std::path::Path,
        network: Network,
        seed: &SecretVec<u8>,
    ) -> Result<(Db, FullViewingKey, SpendAuthorizingKey)> {
        let mut db = WalletDb::for_path(path, network, SystemClock, OsRng)
            .map_err(|e| anyhow!("open wallet db: {e}"))?;
        let usk = UnifiedSpendingKey::from_seed(&network, seed.expose_secret(), AccountId::ZERO)
            .map_err(|e| anyhow!("derive unified spending key: {e}"))?;
        zcash_client_sqlite::wallet::init::init_wallet_db(&mut db, None)
            .map_err(|e| anyhow!("init wallet db: {e}"))?;
        let fvk = FullViewingKey::from(usk.orchard());
        let ask = SpendAuthorizingKey::from(usk.orchard());
        Ok((db, fvk, ask))
    }

    fn from_parts(
        db: Db,
        account_id: AccountUuid,
        fvk: FullViewingKey,
        ask: SpendAuthorizingKey,
    ) -> Self {
        Self {
            db: RefCell::new(db),
            account_id,
            orchard_fvk: fvk,
            orchard_ask: ask,
            zebra_tip: Cell::new(0),
            zebra_tip_hash: Cell::new([0u8; 32]),
        }
    }

    /// Create a NEW wallet. Fails if the wallet/account already exists. Derives
    /// a real birthday from the canonical treestate at `tip - 100` (upstream
    /// reorg buffer).
    pub fn create_new(
        path: &std::path::Path,
        network: Network,
        seed: SecretVec<u8>,
        chain_source: &dyn crate::chain_source::CanonicalChainSource,
    ) -> Result<Self> {
        let (mut db, fvk, ask) = Self::open_db_and_derive(path, network, &seed)?;
        if !db
            .get_account_ids()
            .map_err(|e| anyhow!("account ids: {e}"))?
            .is_empty()
        {
            bail!("wallet already exists; use reopen() or a fresh path");
        }
        let tip = chain_source.canonical_tip()?;
        let birthday_state_height = tip.height.saturating_sub(100);
        let chain_state = chain_source.tree_state(birthday_state_height)?;
        let birthday = AccountBirthday::from_parts(chain_state, None);
        let (account_id, _) = db
            .create_account("zalkanes", &seed, &birthday, None)
            .map_err(|e| anyhow!("create account: {e}"))?;
        Ok(Self::from_parts(db, account_id, fvk, ask))
    }

    /// Reopen an EXISTING wallet. Fails if it does not exist. Preserves the
    /// stored birthday, scanned progress, account identity, and reservations;
    /// does NOT recompute the birthday from the current Zebra tip.
    pub fn reopen(path: &std::path::Path, network: Network, seed: SecretVec<u8>) -> Result<Self> {
        let (db, fvk, ask) = Self::open_db_and_derive(path, network, &seed)?;
        let ids = db
            .get_account_ids()
            .map_err(|e| anyhow!("account ids: {e}"))?;
        let account_id = ids
            .first()
            .copied()
            .ok_or_else(|| anyhow!("wallet does not exist; use create_new() or restore()"))?;
        Ok(Self::from_parts(db, account_id, fvk, ask))
    }

    /// Restore a wallet from a seed at an EXPLICIT birthday height. Fails if the
    /// wallet already exists. Never guesses the birthday: the caller must supply
    /// the first block height to scan; the treestate for the block immediately
    /// before it is fetched from Zebra.
    pub fn restore(
        path: &std::path::Path,
        network: Network,
        seed: SecretVec<u8>,
        birthday_height: u32,
        chain_source: &dyn crate::chain_source::CanonicalChainSource,
    ) -> Result<Self> {
        if birthday_height == 0 {
            bail!("restore birthday height must be >= 1");
        }
        let (mut db, fvk, ask) = Self::open_db_and_derive(path, network, &seed)?;
        if !db
            .get_account_ids()
            .map_err(|e| anyhow!("account ids: {e}"))?
            .is_empty()
        {
            bail!("wallet already exists; use reopen() or a fresh path");
        }
        let chain_state = chain_source.tree_state(birthday_height - 1)?;
        let birthday = AccountBirthday::from_parts(chain_state, None);
        let (account_id, _) = db
            .create_account("zalkanes", &seed, &birthday, None)
            .map_err(|e| anyhow!("restore account: {e}"))?;
        Ok(Self::from_parts(db, account_id, fvk, ask))
    }

    /// Update the canonical Zebra tip (height + hash) this wallet plans against.
    pub fn set_canonical_tip(&self, tip: crate::funding::CanonicalTip) {
        self.zebra_tip.set(tip.height);
        self.zebra_tip_hash.set(tip.hash);
    }

    /// Scan from the wallet's current tip through `chain_source`'s canonical tip
    /// using the canonical full-block path (`decrypt_block` -> `scan_block` ->
    /// `put_blocks`). Rejects (and leaves the wallet unsynced) if the canonical
    /// chain identity changed while scanning.
    pub fn scan_to_tip(
        &self,
        chain_source: &dyn crate::chain_source::CanonicalChainSource,
    ) -> Result<()> {
        let target = chain_source.canonical_tip()?;
        let network = *self.db.borrow().params();

        // Compute everything requiring an immutable borrow BEFORE acquiring the
        // mutable borrow (a nested borrow would panic the RefCell).
        // `prev_hash` is `Some` when the wallet has a stored prior scanned block
        // (an existing wallet), and `None` for a fresh wallet (continuity is then
        // enforced against the birthday ChainState by put_blocks).
        let (ufvks, nullifiers, start_height, mut prev_hash, birthday) = {
            let db = self.db.borrow();
            let ufvks = db
                .get_unified_full_viewing_keys()
                .map_err(|e| anyhow!("ufvks: {e}"))?;
            let nullifiers = Nullifiers::unspent(&*db).map_err(|e| anyhow!("nullifiers: {e}"))?;
            let birthday = u32::from(
                db.get_account_birthday(self.account_id)
                    .map_err(|e| anyhow!("account birthday: {e}"))?,
            );
            let (start_height, prev_hash): (u32, Option<[u8; 32]>) = match db
                .block_fully_scanned()
                .map_err(|e| anyhow!("block_fully_scanned: {e}"))?
            {
                Some(meta) => (
                    u32::from(meta.block_height()) + 1,
                    Some(meta.block_hash().0),
                ),
                None => (birthday, None),
            };
            (ufvks, nullifiers, start_height, prev_hash, birthday)
        };
        let scanning_keys = ScanningKeys::from_account_ufvks(ufvks);

        let mut db = self.db.borrow_mut();
        let mut nullifiers = nullifiers;
        let mut height = start_height;

        while height <= target.height {
            let from_state = chain_source.tree_state(height - 1)?;
            // Reorg check: if the wallet has a stored prior hash, it MUST match
            // Zebra at the same height before scanning forward — including the
            // first new block of an existing wallet.
            if let Some(prev) = prev_hash {
                if from_state.block_hash().0 != prev {
                    // Reorg: find the highest common ancestor using wallet-stored
                    // scanned hashes vs our Zebra, rewind canonically, then resume.
                    height =
                        rewind_to_common_ancestor(&mut db, chain_source, height - 1, birthday)?;
                    nullifiers = Nullifiers::unspent(&*db)
                        .map_err(|e| anyhow!("nullifiers after rewind: {e}"))?;
                    prev_hash = Some(chain_source.block_hash(height - 1)?);
                    continue;
                }
            }

            let block = chain_source.block(height)?;
            let prior_metadata = chain_state_to_block_metadata(&from_state);
            let (header, vtx) = scanning::full::decrypt_block(&network, block, &scanning_keys);
            let scanned = scanning::full::scan_block(
                &network,
                BlockHeight::from_u32(height),
                &header,
                vtx,
                &scanning_keys,
                &nullifiers,
                Some(&prior_metadata),
                |_addr| Ok::<_, std::convert::Infallible>(None),
            )
            .map_err(|e| anyhow!("scan block {height}: {e:?}"))?;
            nullifiers.update_with(&scanned);

            db.put_blocks(&from_state, vec![scanned])
                .map_err(|e| anyhow!("put block {height}: {e}"))?;
            prev_hash = Some(chain_source.block_hash(height)?);
            height += 1;
        }

        // Tell the wallet the canonical chain tip (confirmations/spendability),
        // separately from scan progress; sync is judged by fully_scanned() above.
        db.update_chain_tip(BlockHeight::from_u32(target.height))
            .map_err(|e| anyhow!("update_chain_tip: {e}"))?;

        // Post-scan re-verification: the canonical tip must be unchanged.
        let final_hash = chain_source.block_hash(target.height)?;
        if final_hash != target.hash {
            bail!(
                "canonical tip changed while scanning ({} != {}); re-run sync",
                hex::encode(final_hash),
                hex::encode(target.hash)
            );
        }
        self.set_canonical_tip(target);
        Ok(())
    }

    /// The next block height to scan (first fully-unscanned block): the last
    /// fully-scanned block + 1, or the account birthday height if nothing has
    /// been fully scanned yet. Never genesis/block 1 for a fresh account.
    pub fn next_scan_height(&self) -> Result<u32> {
        let db = self.db.borrow();
        match db
            .block_fully_scanned()
            .map_err(|e| anyhow!("block_fully_scanned: {e}"))?
        {
            Some(meta) => Ok(u32::from(meta.block_height()) + 1),
            None => {
                let birthday = db
                    .get_account_birthday(self.account_id)
                    .map_err(|e| anyhow!("account birthday: {e}"))?;
                Ok(u32::from(birthday))
            }
        }
    }

    /// The account's birthday height (the first block to scan).
    pub fn birthday_height(&self) -> Result<u32> {
        let db = self.db.borrow();
        Ok(u32::from(
            db.get_account_birthday(self.account_id)
                .map_err(|e| anyhow!("account birthday: {e}"))?,
        ))
    }

    pub fn account_id(&self) -> AccountUuid {
        self.account_id
    }

    /// The Unified Address of the account (shielded receivers only).
    pub fn unified_address(&self) -> Result<String> {
        let db = self.db.borrow();
        let ufvks = db
            .get_unified_full_viewing_keys()
            .map_err(|e| anyhow!("unified viewing keys: {e}"))?;
        let ufvk = ufvks
            .get(&self.account_id)
            .ok_or_else(|| anyhow!("account has no unified viewing key"))?;
        let (ua, _) = ufvk
            .default_address(UnifiedAddressRequest::SHIELDED)
            .map_err(|e| anyhow!("default address: {e:?}"))?;
        Ok(UnifiedAddress::encode(&ua, db.params()))
    }

    /// The wallet's fully-scanned progress (height + block hash), or `None` if
    /// nothing has been fully scanned yet. Uses the upstream fully-scanned block
    /// metadata, NOT the known chain tip.
    fn fully_scanned(&self) -> Result<Option<(u32, [u8; 32])>> {
        let db = self.db.borrow();
        match db
            .block_fully_scanned()
            .map_err(|e| anyhow!("block_fully_scanned: {e}"))?
        {
            Some(meta) => Ok(Some((u32::from(meta.block_height()), meta.block_hash().0))),
            None => Ok(None),
        }
    }

    /// A one-line summary of the wallet's unspent shielded notes by pool.
    pub fn shielded_note_summary(&self) -> Result<String> {
        let mut db = self.db.borrow_mut();
        let notes = db
            .select_unspent_notes(
                self.account_id,
                &[ShieldedPool::Orchard, ShieldedPool::Ironwood],
                TargetHeight::from(BlockHeight::from_u32(self.zebra_tip.get())),
                &[],
                LockFilter::Policy(&LockedInputPolicy::Exclude),
            )
            .map_err(|e| anyhow!("unspent notes: {e:?}"))?;
        Ok(format!(
            "orchard_notes={} ironwood_notes={}",
            notes.orchard().len(),
            notes.ironwood().len()
        ))
    }

    /// The account's spendable balance (all pools), in zat.
    pub fn balance(&self) -> Result<u64> {
        let db = self.db.borrow();
        let summary = db
            .get_wallet_summary(ConfirmationsPolicy::MIN)
            .map_err(|e| anyhow!("wallet summary: {e}"))?;
        let Some(summary) = summary else {
            return Ok(0);
        };
        let balances = summary.account_balances();
        Ok(balances
            .get(&self.account_id)
            .map_or(0, |b| u64::from(b.spendable_value())))
    }

    fn lock_owner_for(plan_id: &str) -> LockOwner {
        let mut h = Sha256::new();
        h.update(b"zalkanes-plan-lock\0");
        h.update(plan_id.as_bytes());
        let digest = h.finalize();
        let mut bytes = [0u8; 32];
        bytes.copy_from_slice(&digest);
        LockOwner::new(bytes)
    }
}

impl ShieldedWallet for SqliteShieldedWallet {
    fn sync_status(&self, tip: crate::funding::CanonicalTip) -> Result<SyncStatus> {
        let (scan_height, scan_hash) = self.fully_scanned()?.unwrap_or((0, [0u8; 32]));
        // Spending is safe only when height AND hash both match the canonical
        // chain identity (a same-height reorg changes the hash).
        let synced = scan_height == tip.height && scan_hash == tip.hash;
        Ok(SyncStatus {
            zebra_tip_height: tip.height,
            zebra_tip_hash: tip.hash,
            wallet_scan_height: scan_height,
            wallet_scan_hash: scan_hash,
            synced,
            anchor_height: if synced { Some(tip.height) } else { None },
        })
    }

    fn select_spends(&self, required_zat: u64, plan_id: &str) -> Result<ShieldedSelection> {
        let zebra_tip = self.zebra_tip.get();
        let zebra_tip_hash = self.zebra_tip_hash.get();
        if zebra_tip == 0 {
            bail!("zebra tip unknown; run sync before planning");
        }
        let status = self.sync_status(crate::funding::CanonicalTip {
            height: zebra_tip,
            hash: zebra_tip_hash,
        })?;
        if !status.synced {
            bail!(
                "shielded wallet not synced (zebra {}:{}, wallet {}:{}); \
                 refuse to plan from stale witness state",
                status.zebra_tip_height,
                hex::encode(status.zebra_tip_hash),
                status.wallet_scan_height,
                hex::encode(status.wallet_scan_hash),
            );
        }

        let target_height = BlockHeight::from_u32(zebra_tip + 1);
        let anchor_height = BlockHeight::from_u32(zebra_tip);

        let mut db = self.db.borrow_mut();
        let notes = db
            .select_spendable_notes(
                self.account_id,
                TargetValue::AtLeast(zcash_protocol::value::Zatoshis::from_u64(required_zat)?),
                &[ShieldedPool::Orchard, ShieldedPool::Ironwood],
                TargetHeight::from(target_height),
                ConfirmationsPolicy::MIN,
                &[],
                LockFilter::Policy(&LockedInputPolicy::Exclude),
            )
            .map_err(|e| anyhow!("select spendable notes: {e:?}"))?;

        let mut note_list: Vec<(ValuePool, ReceivedNote<ReceivedNoteId, orchard::Note>)> =
            Vec::new();
        for rn in notes.orchard() {
            note_list.push((ValuePool::Orchard, rn.clone()));
        }
        for rn in notes.ironwood() {
            note_list.push((ValuePool::Ironwood, rn.clone()));
        }
        if note_list.is_empty() {
            bail!("no spendable shielded notes covering {required_zat} zat");
        }

        let mut spends = Vec::with_capacity(note_list.len());
        let mut selected_value = 0u64;
        let mut output_refs = Vec::with_capacity(note_list.len());
        for (pool, rn) in &note_list {
            let position = rn.note_commitment_tree_position();
            // `with_ironwood_tree_mut` returns `Option<A>` (the tree may be
            // absent), so flatten its nested Option; `with_orchard_tree_mut`
            // returns `A` directly.
            let w = match pool {
                ValuePool::Orchard => db
                    .with_orchard_tree_mut(|tree| {
                        tree.witness_at_checkpoint_id_caching(position, &anchor_height)
                    })
                    .map_err(|e| anyhow!("orchard witness: {e}"))?,
                ValuePool::Ironwood => db
                    .with_ironwood_tree_mut(|tree| {
                        tree.witness_at_checkpoint_id_caching(position, &anchor_height)
                    })
                    .map_err(|e| anyhow!("ironwood witness: {e}"))?
                    .flatten(),
            };
            let merkle_path = w
                .ok_or_else(|| anyhow!("no witness at anchor {anchor_height}"))?
                .into();
            let note = *rn.note();
            let value = note.value().inner();
            selected_value += value;
            output_refs.push(OutputRef::new(
                *rn.txid(),
                zcash_protocol::PoolType::Shielded(pool_to_shielded_pool(*pool)),
                u32::from(rn.output_index()),
            ));
            spends.push(ShieldedSpend {
                fvk: self.orchard_fvk.clone(),
                ask: self.orchard_ask.clone(),
                note,
                merkle_path,
                value,
                pool: *pool,
            });
        }

        let mut anchor_for = |pool: ValuePool| -> Result<Option<Anchor>> {
            let root = match pool {
                ValuePool::Orchard => db
                    .with_orchard_tree_mut(|tree| tree.root_at_checkpoint_id(&anchor_height))
                    .map_err(|e| anyhow!("orchard anchor: {e}"))?,
                ValuePool::Ironwood => db
                    .with_ironwood_tree_mut(|tree| tree.root_at_checkpoint_id(&anchor_height))
                    .map_err(|e| anyhow!("ironwood anchor: {e}"))?
                    .flatten(),
            };
            Ok(root.map(|r| r.into()))
        };
        let orchard_anchor = anchor_for(ValuePool::Orchard)?;
        let ironwood_anchor = anchor_for(ValuePool::Ironwood)?;

        // Atomically reserve the selected notes under this plan's lock owner.
        let owner = Self::lock_owner_for(plan_id);
        let lock_expiry = BlockHeight::from_u32(zebra_tip + 128);
        db.lock_outputs(&output_refs, owner, lock_expiry)
            .map_err(|e| anyhow!("reserve notes (lock conflict?): {e:?}"))?;

        let change_address = self
            .orchard_fvk
            .address_at(0u32, orchard::keys::Scope::Internal);

        Ok(ShieldedSelection {
            spends,
            selected_value,
            change_address,
            change_fvk: self.orchard_fvk.clone(),
            change_ovk: Some(self.orchard_fvk.to_ovk(orchard::keys::Scope::Internal)),
            change_pool: ValuePool::Orchard,
            orchard_anchor,
            ironwood_anchor,
            output_refs,
            lock_owner: owner,
        })
    }

    fn release(&self, plan_id: &str) -> Result<()> {
        let owner = Self::lock_owner_for(plan_id);
        let mut db = self.db.borrow_mut();
        let locked = db
            .get_locked_outputs(self.account_id)
            .map_err(|e| anyhow!("locked outputs: {e}"))?;
        for r in locked {
            let _ = db.unlock_output(&r, owner);
        }
        Ok(())
    }
}

/// Derive a `BlockMetadata` (tree sizes) from a canonical `ChainState`.
fn chain_state_to_block_metadata(cs: &ChainState) -> BlockMetadata {
    BlockMetadata::from_parts(
        cs.block_height(),
        cs.block_hash(),
        Some(u32::try_from(cs.final_sapling_tree().tree_size()).unwrap_or(0)),
        Some(u32::try_from(cs.final_orchard_tree().tree_size()).unwrap_or(0)),
        Some(u32::try_from(cs.final_ironwood_tree().tree_size()).unwrap_or(0)),
    )
}

/// Find the highest common ancestor between the wallet's scanned chain and our
/// Zebra, rewind the wallet to it via the canonical `rewind_to_chain_state`, and
/// return the next block height to scan (ancestor + 1). Bounded by `birthday`;
/// a reorg that crosses the wallet birthday is an error (requires re-restore).
fn rewind_to_common_ancestor(
    db: &mut Db,
    chain_source: &dyn crate::chain_source::CanonicalChainSource,
    mut height: u32,
    birthday: u32,
) -> Result<u32> {
    loop {
        let wallet_hash = db
            .get_block_hash(BlockHeight::from_u32(height))
            .map_err(|e| anyhow!("wallet block hash {height}: {e}"))?
            .map(|h| h.0);
        let zebra_hash = chain_source.block_hash(height)?;
        if wallet_hash == Some(zebra_hash) {
            let chain_state = chain_source.tree_state(height)?;
            db.rewind_to_chain_state(chain_state, std::collections::HashSet::new())
                .map_err(|e| anyhow!("rewind_to_chain_state({height}): {e:?}"))?;
            return Ok(height + 1);
        }
        if height <= birthday {
            bail!(
                "reorg crosses wallet birthday {birthday}; refuse to rewind further \
                 (re-restore with an explicit earlier birthday required)"
            );
        }
        height -= 1;
    }
}

/// Map an Orchard-protocol value pool to the protocol-level shielded pool.
fn pool_to_shielded_pool(pool: ValuePool) -> ShieldedPool {
    match pool {
        ValuePool::Orchard => ShieldedPool::Orchard,
        ValuePool::Ironwood => ShieldedPool::Ironwood,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand_core::RngCore;
    use zcash_client_backend::data_api::chain::ChainState;
    use zcash_primitives::block::{Block, BlockHash};

    fn random_seed() -> SecretVec<u8> {
        let mut bytes = vec![0u8; 32];
        OsRng.fill_bytes(&mut bytes);
        SecretVec::new(bytes)
    }

    /// A mock chain source returning an empty treestate (unit test only).
    struct MockChainSource {
        tip: u32,
    }
    impl crate::funding::TipSource for MockChainSource {
        fn canonical_tip(&self) -> Result<crate::funding::CanonicalTip> {
            Ok(crate::funding::CanonicalTip {
                height: self.tip,
                hash: [0u8; 32],
            })
        }
    }
    impl crate::chain_source::CanonicalChainSource for MockChainSource {
        fn block_hash(&self, _h: u32) -> Result<[u8; 32]> {
            Ok([0u8; 32])
        }
        fn block(&self, _h: u32) -> Result<Block> {
            unreachable!("mock block")
        }
        fn tree_state(&self, h: u32) -> Result<ChainState> {
            Ok(ChainState::empty(
                BlockHeight::from_u32(h),
                BlockHash([0u8; 32]),
            ))
        }
    }

    #[test]
    fn open_or_create_derives_account_and_address() {
        let mut suffix = [0u8; 8];
        OsRng.fill_bytes(&mut suffix);
        let path = std::env::temp_dir().join(format!(
            "zalkanes-wallet-test-{}.sqlite",
            hex::encode(suffix)
        ));

        let wallet = SqliteShieldedWallet::create_new(
            &path,
            Network::TestNetwork,
            random_seed(),
            &MockChainSource { tip: 200 },
        )
        .unwrap();

        let ua = wallet.unified_address().unwrap();
        assert!(!ua.is_empty());
        // A freshly created, never-scanned wallet has no spendable balance.
        assert_eq!(wallet.balance().unwrap(), 0);
        // Without a Zebra tip, sync must report unsynced.
        let status = wallet
            .sync_status(crate::funding::CanonicalTip {
                height: 1_000,
                hash: [0u8; 32],
            })
            .unwrap();
        assert!(!status.synced);
        assert_eq!(status.wallet_scan_height, 0);

        drop(wallet);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn fresh_wallet_first_scan_starts_at_birthday_not_genesis() {
        let mut suffix = [0u8; 8];
        OsRng.fill_bytes(&mut suffix);
        let path = std::env::temp_dir().join(format!(
            "zalkanes-wallet-firstscan-{}.sqlite",
            hex::encode(suffix)
        ));

        // Tip 10_000 -> birthday prior state 9_900 -> first scan block 9_901.
        let wallet = SqliteShieldedWallet::create_new(
            &path,
            Network::TestNetwork,
            random_seed(),
            &MockChainSource { tip: 10_000 },
        )
        .unwrap();
        let start = wallet.next_scan_height().unwrap();
        assert_eq!(
            start, 9_901,
            "first scan must start at birthday height, not 1"
        );

        drop(wallet);
        let _ = std::fs::remove_file(&path);
    }

    fn tmp_path(tag: &str) -> std::path::PathBuf {
        let mut suffix = [0u8; 8];
        OsRng.fill_bytes(&mut suffix);
        std::env::temp_dir().join(format!("zalkanes-{tag}-{}.sqlite", hex::encode(suffix)))
    }

    #[test]
    fn create_new_twice_fails() {
        let path = tmp_path("create-twice");
        let mock = MockChainSource { tip: 200 };
        SqliteShieldedWallet::create_new(&path, Network::TestNetwork, random_seed(), &mock)
            .unwrap();
        let err =
            SqliteShieldedWallet::create_new(&path, Network::TestNetwork, random_seed(), &mock);
        assert!(
            err.is_err(),
            "second create_new must fail, not silently reopen"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn reopen_preserves_birthday_and_account() {
        let path = tmp_path("reopen");
        let seed = random_seed();
        let mock = MockChainSource { tip: 1_000 };
        let w = SqliteShieldedWallet::create_new(
            &path,
            Network::TestNetwork,
            SecretVec::new(seed.expose_secret().to_vec()),
            &mock,
        )
        .unwrap();
        let id_before = w.account_id();
        let birthday_before = w.birthday_height().unwrap();
        drop(w);

        let w = SqliteShieldedWallet::reopen(&path, Network::TestNetwork, seed).unwrap();
        assert_eq!(w.account_id(), id_before);
        assert_eq!(w.birthday_height().unwrap(), birthday_before);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn reopen_missing_db_fails() {
        let path = tmp_path("reopen-missing");
        let err = SqliteShieldedWallet::reopen(&path, Network::TestNetwork, random_seed());
        assert!(err.is_err(), "reopen on missing db must fail");
    }

    #[test]
    fn restore_uses_explicit_birthday() {
        let path = tmp_path("restore");
        let mock = MockChainSource { tip: 10_000 };
        let w =
            SqliteShieldedWallet::restore(&path, Network::TestNetwork, random_seed(), 5_000, &mock)
                .unwrap();
        assert_eq!(
            w.birthday_height().unwrap(),
            5_000,
            "restore must honor the explicit birthday"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn lock_owner_is_deterministic() {
        let a = SqliteShieldedWallet::lock_owner_for("plan-1");
        let b = SqliteShieldedWallet::lock_owner_for("plan-1");
        let c = SqliteShieldedWallet::lock_owner_for("plan-2");
        assert_eq!(a, b);
        assert_ne!(a, c);
    }
}
