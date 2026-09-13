//! zalkanes-dex-sdk — user-facing SUBFROST AMM API.
//!
//! Two layers:
//!
//! 1. **Intents** ([`SwapIntent`], [`AddLiquidityIntent`], …): what the
//!    user wants, independent of transport. `build_*` functions turn an
//!    intent into a [`CallPlan`] — the exact `(contract, opcode,
//!    calldata, attached assets)` tuple a Zalkanes CALL must carry.
//!    No Zcash consensus/wire structure is defined here; converting a
//!    `CallPlan` into an actual Zcash transaction is the platform
//!    transport's job (blocked upstream until the DEX host ABI lands).
//!
//! 2. **Views** ([`DexView`]): read APIs over any transport implementing
//!    [`ViewTransport`] (the DEX testkit implements it; a JSON-RPC
//!    transport can implement it later without touching this crate).

#![forbid(unsafe_code)]

use zalkanes_dex_core::encode::{
    self, decode_pool_details, decode_pool_list, decode_reserves, decode_swap_quote, factory_op,
    pool_op,
};
use zalkanes_dex_core::error::DexError;
use zalkanes_dex_core::math;
use zalkanes_dex_core::types::{AssetId, ContractId, Holder, Pair, PoolDetails, SwapQuote};

/// A fully specified DEX contract call, ready for a transport.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallPlan {
    pub contract: ContractId,
    pub opcode: u16,
    pub input: Vec<u8>,
    /// Assets the call must attach (canonically sorted, nonzero).
    pub assets: Vec<(AssetId, u128)>,
}

impl CallPlan {
    fn new(
        contract: ContractId,
        opcode: u16,
        input: Vec<u8>,
        mut assets: Vec<(AssetId, u128)>,
    ) -> Self {
        assets.sort_by(|a, b| a.0.cmp(&b.0));
        CallPlan {
            contract,
            opcode,
            input,
            assets,
        }
    }
}

// ── intents ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CreatePoolIntent {
    pub factory: ContractId,
    pub token_a: AssetId,
    pub token_b: AssetId,
    pub amount_a: u128,
    pub amount_b: u128,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AddLiquidityIntent {
    pub pool: ContractId,
    pub token0: AssetId,
    pub token1: AssetId,
    pub desired0: u128,
    pub desired1: u128,
    pub min_lp_out: u128,
    pub expiry_height: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemoveLiquidityIntent {
    pub pool: ContractId,
    pub lp_amount: u128,
    pub min_amount0: u128,
    pub min_amount1: u128,
    pub expiry_height: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SwapIntent {
    pub pool: ContractId,
    pub incoming_asset: AssetId,
    pub incoming_amount: u128,
    pub min_amount_out: u128,
    pub expiry_height: u32,
}

pub fn build_create_pool(intent: &CreatePoolIntent) -> Result<CallPlan, DexError> {
    Pair::canonical(intent.token_a, intent.token_b)?;
    if intent.amount_a == 0 || intent.amount_b == 0 {
        return Err(DexError::ZeroAmount);
    }
    Ok(CallPlan::new(
        intent.factory,
        factory_op::CREATE_POOL,
        encode::factory_create_pool_args(&intent.token_a, &intent.token_b),
        vec![
            (intent.token_a, intent.amount_a),
            (intent.token_b, intent.amount_b),
        ],
    ))
}

pub fn build_add_liquidity(intent: &AddLiquidityIntent) -> Result<CallPlan, DexError> {
    if intent.desired0 == 0 || intent.desired1 == 0 {
        return Err(DexError::ZeroAmount);
    }
    if intent.token0 >= intent.token1 {
        return Err(DexError::InvalidArguments);
    }
    Ok(CallPlan::new(
        intent.pool,
        pool_op::ADD_LIQUIDITY,
        encode::pool_add_liquidity_args(intent.min_lp_out, intent.expiry_height),
        vec![
            (intent.token0, intent.desired0),
            (intent.token1, intent.desired1),
        ],
    ))
}

pub fn build_remove_liquidity(intent: &RemoveLiquidityIntent) -> Result<CallPlan, DexError> {
    if intent.lp_amount == 0 {
        return Err(DexError::ZeroAmount);
    }
    Ok(CallPlan::new(
        intent.pool,
        pool_op::REMOVE_LIQUIDITY,
        encode::pool_remove_liquidity_args(
            intent.min_amount0,
            intent.min_amount1,
            intent.expiry_height,
        ),
        vec![(AssetId::of_contract(&intent.pool), intent.lp_amount)],
    ))
}

pub fn build_swap_exact_in(intent: &SwapIntent) -> Result<CallPlan, DexError> {
    if intent.incoming_amount == 0 {
        return Err(DexError::ZeroAmount);
    }
    Ok(CallPlan::new(
        intent.pool,
        pool_op::SWAP_EXACT_IN,
        encode::pool_swap_args(intent.min_amount_out, intent.expiry_height),
        vec![(intent.incoming_asset, intent.incoming_amount)],
    ))
}

// ── views ────────────────────────────────────────────────────────────────

/// Read-only access to DEX contracts. The DEX testkit implements this;
/// future transports (JSON-RPC against a live indexer) implement it too.
pub trait ViewTransport {
    fn view(&self, contract: ContractId, opcode: u16, input: &[u8]) -> Result<Vec<u8>, DexError>;
}

pub struct DexView<'a, T: ViewTransport + ?Sized> {
    transport: &'a T,
    factory: ContractId,
}

impl<'a, T: ViewTransport + ?Sized> DexView<'a, T> {
    pub fn new(transport: &'a T, factory: ContractId) -> Self {
        Self { transport, factory }
    }

    /// Pool for the unordered pair, if one exists.
    pub fn get_pool(&self, a: AssetId, b: AssetId) -> Result<Option<ContractId>, DexError> {
        let out = self.transport.view(
            self.factory,
            factory_op::GET_POOL,
            &encode::factory_get_pool_args(&a, &b),
        )?;
        if out.is_empty() {
            return Ok(None);
        }
        if out.len() != 32 {
            return Err(DexError::InvalidArguments);
        }
        let mut id = [0u8; 32];
        id.copy_from_slice(&out);
        Ok(Some(ContractId(id)))
    }

    /// All pools in deterministic creation order.
    pub fn get_all_pools(&self) -> Result<Vec<ContractId>, DexError> {
        decode_pool_list(
            &self
                .transport
                .view(self.factory, factory_op::GET_ALL_POOLS, &[])?,
        )
    }

    pub fn pool_count(&self) -> Result<u128, DexError> {
        let out = self
            .transport
            .view(self.factory, factory_op::POOL_COUNT, &[])?;
        if out.len() != 16 {
            return Err(DexError::InvalidArguments);
        }
        let mut buf = [0u8; 16];
        buf.copy_from_slice(&out);
        Ok(u128::from_be_bytes(buf))
    }

    pub fn pool_details(&self, pool: ContractId) -> Result<PoolDetails, DexError> {
        decode_pool_details(&self.transport.view(pool, pool_op::POOL_DETAILS, &[])?)
    }

    pub fn get_reserves(&self, pool: ContractId) -> Result<(u128, u128), DexError> {
        decode_reserves(&self.transport.view(pool, pool_op::GET_RESERVES, &[])?)
    }

    /// Contract-evaluated quote (authoritative).
    pub fn quote_exact_in(
        &self,
        pool: ContractId,
        token_in: AssetId,
        amount_in: u128,
    ) -> Result<SwapQuote, DexError> {
        decode_swap_quote(&self.transport.view(
            pool,
            pool_op::QUOTE_EXACT_IN,
            &encode::pool_quote_args(&token_in, amount_in),
        )?)
    }

    /// Client-side quote from fetched reserves; must equal the contract
    /// quote for the same state (both use the frozen v0 math).
    pub fn quote_exact_in_local(
        &self,
        pool: ContractId,
        token_in: AssetId,
        amount_in: u128,
    ) -> Result<SwapQuote, DexError> {
        let details = self.pool_details(pool)?;
        let (reserve_in, reserve_out, token_out) = if token_in == details.token0 {
            (details.reserve0, details.reserve1, details.token1)
        } else if token_in == details.token1 {
            (details.reserve1, details.reserve0, details.token0)
        } else {
            return Err(DexError::InvalidAsset);
        };
        let outcome = math::quote_swap_exact_in(reserve_in, reserve_out, amount_in)?;
        Ok(SwapQuote {
            token_in,
            token_out,
            amount_in,
            amount_out: outcome.amount_out,
            total_fee: outcome.fees.total_fee,
            lp_fee: outcome.fees.lp_fee,
            protocol_fee: outcome.fees.protocol_fee,
        })
    }
}

/// Convenience: the holder a swap's output lands on (documentation aid).
#[must_use]
pub fn output_recipient(caller: Holder) -> Holder {
    caller
}
