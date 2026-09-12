//! Pure, deterministic AMM arithmetic for SUBFROST v0.
//!
//! Every function is a total function of its integer inputs: no floating
//! point, no host state, no randomness, no time. All rounding is explicit
//! floor. All overflow paths return errors; nothing panics on any input.

use crate::error::DexError;
use crate::v0::{FEE_DENOMINATOR_BPS, LP_FEE_BPS, MINIMUM_LIQUIDITY, TOTAL_SWAP_FEE_BPS};
use crate::wide::{mul_div_floor, mul_wide, sqrt_wide};

/// Fee decomposition of a swap input amount.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FeeSplit {
    pub total_fee: u128,
    pub lp_fee: u128,
    pub protocol_fee: u128,
}

/// Split `amount_in` into fee components. All divisions floor.
///
/// `protocol_fee = total_fee - lp_fee`, so the three parts always satisfy
/// `lp_fee + protocol_fee == total_fee` exactly.
pub fn split_fee(amount_in: u128) -> Result<FeeSplit, DexError> {
    let total_fee = mul_div_floor(amount_in, TOTAL_SWAP_FEE_BPS, FEE_DENOMINATOR_BPS)?;
    let lp_fee = mul_div_floor(amount_in, LP_FEE_BPS, FEE_DENOMINATOR_BPS)?;
    let protocol_fee = total_fee
        .checked_sub(lp_fee)
        .ok_or(DexError::ArithmeticUnderflow)?;
    Ok(FeeSplit {
        total_fee,
        lp_fee,
        protocol_fee,
    })
}

/// Initial LP issuance: `gross = floor(sqrt(amount0 * amount1))` with a
/// 256-bit product, minus the permanently locked [`MINIMUM_LIQUIDITY`].
///
/// Returns `(provider_lp, gross_lp)`; `gross_lp` becomes the total supply.
pub fn initial_liquidity(amount0: u128, amount1: u128) -> Result<(u128, u128), DexError> {
    if amount0 == 0 || amount1 == 0 {
        return Err(DexError::ZeroAmount);
    }
    let (hi, lo) = mul_wide(amount0, amount1);
    let gross = sqrt_wide(hi, lo);
    if gross <= MINIMUM_LIQUIDITY {
        return Err(DexError::InsufficientInitialLiquidity);
    }
    Ok((gross - MINIMUM_LIQUIDITY, gross))
}

/// Result of an add-liquidity quote.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AddLiquidityOutcome {
    pub accepted0: u128,
    pub accepted1: u128,
    pub refund0: u128,
    pub refund1: u128,
    pub lp_minted: u128,
}

/// Quote a proportional liquidity add against live reserves.
///
/// Ratio rule (v0):
///   if desired0 * reserve1 <= desired1 * reserve0:
///       accept all of desired0, scale desired1 down
///   else:
///       accept all of desired1, scale desired0 down
/// LP minted = min(floor(a0*S/R0), floor(a1*S/R1)).
pub fn quote_add_liquidity(
    reserve0: u128,
    reserve1: u128,
    total_supply: u128,
    desired0: u128,
    desired1: u128,
) -> Result<AddLiquidityOutcome, DexError> {
    if reserve0 == 0 || reserve1 == 0 || total_supply == 0 {
        return Err(DexError::NotInitialized);
    }
    if desired0 == 0 || desired1 == 0 {
        return Err(DexError::ZeroAmount);
    }
    // Upstream (oyl-amm factory `add_liquidity`) branch rule: try the
    // token1-optimal amount first; fall back to scaling token0 down.
    let optimal1 = mul_div_floor(desired0, reserve1, reserve0)?;
    let (accepted0, accepted1) = if optimal1 <= desired1 {
        (desired0, optimal1)
    } else {
        (mul_div_floor(desired1, reserve0, reserve1)?, desired1)
    };
    let lp0 = mul_div_floor(accepted0, total_supply, reserve0)?;
    let lp1 = mul_div_floor(accepted1, total_supply, reserve1)?;
    let lp_minted = lp0.min(lp1);
    if lp_minted == 0 {
        return Err(DexError::InsufficientLiquidity);
    }
    let refund0 = desired0
        .checked_sub(accepted0)
        .ok_or(DexError::ArithmeticUnderflow)?;
    let refund1 = desired1
        .checked_sub(accepted1)
        .ok_or(DexError::ArithmeticUnderflow)?;
    Ok(AddLiquidityOutcome {
        accepted0,
        accepted1,
        refund0,
        refund1,
        lp_minted,
    })
}

/// Quote a proportional liquidity removal.
///
/// `amount_i = floor(lp_amount * reserve_i / total_supply)`.
pub fn quote_remove_liquidity(
    reserve0: u128,
    reserve1: u128,
    total_supply: u128,
    lp_amount: u128,
) -> Result<(u128, u128), DexError> {
    if total_supply == 0 {
        return Err(DexError::NotInitialized);
    }
    if lp_amount == 0 {
        return Err(DexError::ZeroAmount);
    }
    if lp_amount > total_supply {
        return Err(DexError::InsufficientLp);
    }
    let amount0 = mul_div_floor(lp_amount, reserve0, total_supply)?;
    let amount1 = mul_div_floor(lp_amount, reserve1, total_supply)?;
    // Upstream parity ("INSUFFICIENT_LIQUIDITY_BURNED"): both outputs must
    // be nonzero, or the burn would destroy LP value.
    if amount0 == 0 || amount1 == 0 {
        return Err(DexError::InsufficientLiquidity);
    }
    Ok((amount0, amount1))
}

/// Result of an exact-input swap quote.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SwapOutcome {
    pub amount_out: u128,
    pub fees: FeeSplit,
}

/// Constant-product exact-input swap quote (v0 formula, upstream parity).
///
/// Pricing is the upstream `get_amount_out` (oyl-amm `oylswap-library`)
/// generalized from per-1000 to bps — for the frozen v0 fee (100 bps) the
/// two are floor-for-floor identical:
///
/// ```text
/// amount_in_with_fee = amount_in * (10_000 - 100)
/// amount_out = floor(amount_in_with_fee * reserve_out
///                    / (reserve_in * 10_000 + amount_in_with_fee))
/// ```
///
/// Reported fee split (also drives reserve/protocol accounting):
///
/// ```text
/// total_fee    = floor(amount_in * 100 / 10_000)
/// lp_fee       = floor(amount_in *  80 / 10_000)
/// protocol_fee = total_fee - lp_fee
/// ```
///
/// v0 deviation from upstream: a swap whose `total_fee` floors to zero
/// (`amount_in < 100`) is rejected instead of priced fee-free.
///
/// Guarantees on success: `amount_out > 0` and `amount_out < reserve_out`.
pub fn quote_swap_exact_in(
    reserve_in: u128,
    reserve_out: u128,
    amount_in: u128,
) -> Result<SwapOutcome, DexError> {
    if reserve_in == 0 || reserve_out == 0 {
        return Err(DexError::InsufficientLiquidity);
    }
    if amount_in == 0 {
        return Err(DexError::ZeroAmount);
    }
    let fees = split_fee(amount_in)?;
    if fees.total_fee == 0 {
        return Err(DexError::ZeroAmount);
    }
    let amount_in_with_fee = amount_in
        .checked_mul(FEE_DENOMINATOR_BPS - TOTAL_SWAP_FEE_BPS)
        .ok_or(DexError::ArithmeticOverflow)?;
    let denominator = reserve_in
        .checked_mul(FEE_DENOMINATOR_BPS)
        .and_then(|scaled| scaled.checked_add(amount_in_with_fee))
        .ok_or(DexError::ArithmeticOverflow)?;
    let amount_out = mul_div_floor(amount_in_with_fee, reserve_out, denominator)?;
    if amount_out == 0 {
        return Err(DexError::InsufficientLiquidity);
    }
    debug_assert!(amount_out < reserve_out);
    Ok(SwapOutcome { amount_out, fees })
}
