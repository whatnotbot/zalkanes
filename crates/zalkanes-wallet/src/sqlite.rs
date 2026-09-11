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
        locking::{LockOwner, OutputLockStore},
        wallet::{
            input_selection::{LockFilter, LockedInputPolicy},
            ConfirmationsPolicy, TargetHeight,
        },
        AccountBirthday, InputSource, TargetValue, WalletCommitmentTrees, WalletRead, WalletWrite,
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
    /// Opens (or creates and initializes) the wallet at `path`.
    ///
    /// `seed` is the ZIP-32 HD seed (>= 32 bytes). It is held in memory only —
    /// never written to the Zalkanes consensus DB, never logged, never printed.
    /// On first creation an account is derived and tracked; on reopen the
    /// account is looked up and the spending keys re-derived from `seed`.
    pub fn open_or_create(
        path: &std::path::Path,
        network: Network,
        seed: SecretVec<u8>,
        chain_source: &dyn crate::chain_source::CanonicalChainSource,
    ) -> Result<Self> {
        let mut db = WalletDb::for_path(path, network, SystemClock, OsRng)
            .map_err(|e| anyhow!("open wallet db: {e}"))?;

        // Derive the spending keys before handing the seed to the migrator.
        let usk = UnifiedSpendingKey::from_seed(&network, seed.expose_secret(), AccountId::ZERO)
            .map_err(|e| anyhow!("derive unified spending key: {e}"))?;

        // Initialize schema (no seed needed for a fresh wallet). A wallet that
        // already exists is opened in place; migrations that require the seed
        // are surfaced as an error rather than silently re-derived.
        zcash_client_sqlite::wallet::init::init_wallet_db(&mut db, None)
            .map_err(|e| anyhow!("init wallet db: {e}"))?;

        let account_id = match db
            .get_account_ids()
            .map_err(|e| anyhow!("account ids: {e}"))?
        {
            ids if ids.is_empty() => {
                // Real birthday: the canonical treestate at `tip - 100` (upstream
                // reorg buffer), so a reorg cannot drop a newly-targeted payment
                // below the wallet's birthday. The treestate is the block BEFORE
                // the birthday height.
                let tip = chain_source.canonical_tip()?;
                let birthday_state_height = tip.height.saturating_sub(100);
                let chain_state = chain_source.tree_state(birthday_state_height)?;
                let birthday = AccountBirthday::from_parts(chain_state, None);
                let (id, _usk) = db
                    .create_account("zalkanes", &seed, &birthday, None)
                    .map_err(|e| anyhow!("create account: {e}"))?;
                id
            }
            ids => ids[0],
        };

        let orchard_fvk = FullViewingKey::from(usk.orchard());
        let orchard_ask = SpendAuthorizingKey::from(usk.orchard());

        Ok(Self {
            db: RefCell::new(db),
            account_id,
            orchard_fvk,
            orchard_ask,
            zebra_tip: Cell::new(0),
            zebra_tip_hash: Cell::new([0u8; 32]),
        })
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

        let (ufvks, nullifiers) = {
            let db = self.db.borrow();
            let ufvks = db
                .get_unified_full_viewing_keys()
                .map_err(|e| anyhow!("ufvks: {e}"))?;
            (
                ufvks,
                Nullifiers::unspent(&*db).map_err(|e| anyhow!("nullifiers: {e}"))?,
            )
        };
        let scanning_keys = ScanningKeys::from_account_ufvks(ufvks);

        let mut db = self.db.borrow_mut();
        let mut nullifiers = nullifiers;
        let mut height = self.wallet_scan_tip()?.0 + 1;

        while height <= target.height {
            // Reorg check: the wallet's scanned hash at height-1 must still match
            // our Zebra; a mismatch means a fork and we bail to rewind/rescan.
            if height > 0 {
                let prev = BlockHeight::from_u32(height - 1);
                let wallet_hash = db
                    .get_block_hash(prev)
                    .map_err(|e| anyhow!("wallet block hash: {e}"))?
                    .map(|h| h.0)
                    .unwrap_or([0u8; 32]);
                let zebra_hash = chain_source.block_hash(height - 1)?;
                if wallet_hash != zebra_hash {
                    bail!(
                        "reorg detected at height {}: wallet hash {}, zebra hash {}",
                        height - 1,
                        hex::encode(wallet_hash),
                        hex::encode(zebra_hash)
                    );
                }
            }

            let block = chain_source.block(height)?;
            let prior_metadata = db
                .block_metadata(BlockHeight::from_u32(height - 1))
                .map_err(|e| anyhow!("block metadata: {e}"))?;
            let (header, vtx) = scanning::full::decrypt_block(&network, block, &scanning_keys);
            let scanned = scanning::full::scan_block(
                &network,
                BlockHeight::from_u32(height),
                &header,
                vtx,
                &scanning_keys,
                &nullifiers,
                prior_metadata.as_ref(),
                |_addr| Ok::<_, std::convert::Infallible>(None),
            )
            .map_err(|e| anyhow!("scan block {height}: {e:?}"))?;
            nullifiers.update_with(&scanned);

            let from_state = chain_source.tree_state(height - 1)?;
            db.put_blocks(&from_state, vec![scanned])
                .map_err(|e| anyhow!("put block {height}: {e}"))?;
            height += 1;
        }

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

    /// Wallet scan tip (height + hash), or (0, [0;32]) if never scanned.
    fn wallet_scan_tip(&self) -> Result<(u32, [u8; 32])> {
        let db = self.db.borrow();
        let summary = db
            .get_wallet_summary(ConfirmationsPolicy::MIN)
            .map_err(|e| anyhow!("wallet summary: {e}"))?;
        let height = summary.map_or(0, |s| u32::from(s.chain_tip_height()));
        if height == 0 {
            return Ok((0, [0u8; 32]));
        }
        let hash = db
            .get_block_hash(BlockHeight::from_u32(height))
            .map_err(|e| anyhow!("wallet block hash: {e}"))?
            .map(|h| h.0)
            .unwrap_or([0u8; 32]);
        Ok((height, hash))
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
        let (scan_height, scan_hash) = self.wallet_scan_tip()?;
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
    struct MockChainSource;
    impl crate::chain_source::CanonicalChainSource for MockChainSource {
        fn canonical_tip(&self) -> Result<crate::funding::CanonicalTip> {
            Ok(crate::funding::CanonicalTip {
                height: 200,
                hash: [0u8; 32],
            })
        }
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

        let wallet = SqliteShieldedWallet::open_or_create(
            &path,
            Network::TestNetwork,
            random_seed(),
            &MockChainSource,
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
    fn lock_owner_is_deterministic() {
        let a = SqliteShieldedWallet::lock_owner_for("plan-1");
        let b = SqliteShieldedWallet::lock_owner_for("plan-1");
        let c = SqliteShieldedWallet::lock_owner_for("plan-2");
        assert_eq!(a, b);
        assert_ne!(a, c);
    }
}
