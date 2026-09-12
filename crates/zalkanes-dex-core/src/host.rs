//! Proposed DEX host ABI (contract ↔ runtime boundary).
//!
//! The frozen Zalkanes v0 host ABI exposes six functions (storage get /
//! set / delete, block height, input read, output write). A SUBFROST-style
//! AMM additionally requires the capabilities below. This trait is the
//! precise statement of that upstream requirement; the DEX testkit
//! implements it deterministically in memory, and contract logic is
//! written against it so the same code binds to the real host functions
//! once the platform provides them (ADR required — see
//! `docs/subfrost-amm-v0.md` §"Upstream blockers").
//!
//! Determinism contract: every method must be a pure function of chain
//! state and call context. No time, no randomness, no I/O.

use alloc::vec::Vec;

use crate::error::DexError;
use crate::events::DexEvent;
use crate::types::{AssetId, ContractId, Holder};

/// Runtime capabilities available to a DEX contract during one call.
pub trait Host {
    /// The executing contract's own identity.
    fn self_id(&self) -> ContractId;

    /// The direct caller of this call (external account or contract).
    fn caller(&self) -> Holder;

    /// Canonical block height of the executing block. Contracts must use
    /// this, never a caller-supplied height.
    fn block_height(&self) -> u32;

    /// Assets attached to this call. Already credited to the contract's
    /// custody before dispatch; refunded automatically if the call fails.
    /// Canonically sorted by `AssetId`, amounts nonzero, no duplicates.
    fn incoming_assets(&self) -> Vec<(AssetId, u128)>;

    fn storage_get(&self, key: &[u8]) -> Option<Vec<u8>>;
    fn storage_set(&mut self, key: &[u8], value: &[u8]) -> Result<(), DexError>;
    fn storage_delete(&mut self, key: &[u8]) -> Result<(), DexError>;

    /// Move `amount` of `asset` from this contract's custody to `to`.
    fn transfer_out(&mut self, to: &Holder, asset: &AssetId, amount: u128) -> Result<(), DexError>;

    /// Mint `amount` units of this contract's own asset
    /// (`AssetId::of_contract(self_id)`) to `to`.
    fn mint_own_asset(&mut self, to: &Holder, amount: u128) -> Result<(), DexError>;

    /// Burn `amount` units of this contract's own asset from this
    /// contract's custody.
    fn burn_own_asset(&mut self, amount: u128) -> Result<(), DexError>;

    /// Record a structured event. Events of failed calls are discarded.
    fn emit_event(&mut self, event: &DexEvent);

    /// Synchronous cross-contract call, transferring `assets` from this
    /// contract's custody to `target` first. A failed nested call reverts
    /// all of its own effects (storage, ledger, events) before returning.
    fn call(
        &mut self,
        target: &ContractId,
        opcode: u16,
        input: &[u8],
        assets: &[(AssetId, u128)],
    ) -> Result<Vec<u8>, DexError>;

    /// Instantiate a new contract from a registered template.
    fn spawn(&mut self, template_code_hash: &[u8; 32]) -> Result<ContractId, DexError>;

    /// Deterministic fuel accounting; returns `OutOfFuel` when exhausted.
    fn consume_fuel(&mut self, units: u64) -> Result<(), DexError>;
}

/// Parameters for spawning + initializing a pool in one atomic step.
#[derive(Debug, Clone, Copy)]
pub struct PoolInit {
    pub token0: AssetId,
    pub token1: AssetId,
    pub amount0: u128,
    pub amount1: u128,
    pub provider: Holder,
}

/// Narrow spawn abstraction (spec §20). The testkit `Host` is the
/// `TestContractSpawner`; a `RuntimeContractSpawner` becomes possible only
/// once the platform grows a spawn primitive.
pub trait ContractSpawner {
    fn spawn_pool(
        &mut self,
        template_code_hash: &[u8; 32],
        init: PoolInit,
    ) -> Result<ContractId, DexError>;
}

impl<H: Host + ?Sized> ContractSpawner for H {
    fn spawn_pool(
        &mut self,
        template_code_hash: &[u8; 32],
        init: PoolInit,
    ) -> Result<ContractId, DexError> {
        let pool_id = self.spawn(template_code_hash)?;
        let args = crate::encode::pool_initialize_args(&init.token0, &init.token1, &init.provider);
        let assets = [(init.token0, init.amount0), (init.token1, init.amount1)];
        let mut sorted = assets;
        sorted.sort_by(|a, b| a.0.cmp(&b.0));
        self.call(&pool_id, crate::encode::pool_op::INITIALIZE, &args, &sorted)?;
        Ok(pool_id)
    }
}

/// Read a big-endian u128 storage value; absent key reads as 0.
pub fn get_u128<H: Host + ?Sized>(host: &H, key: &[u8]) -> Result<u128, DexError> {
    match host.storage_get(key) {
        None => Ok(0),
        Some(bytes) => {
            if bytes.len() != 16 {
                return Err(DexError::InvalidArguments);
            }
            let mut buf = [0u8; 16];
            buf.copy_from_slice(&bytes);
            Ok(u128::from_be_bytes(buf))
        }
    }
}

/// Write a big-endian u128 storage value.
pub fn set_u128<H: Host + ?Sized>(host: &mut H, key: &[u8], value: u128) -> Result<(), DexError> {
    host.storage_set(key, &value.to_be_bytes())
}
