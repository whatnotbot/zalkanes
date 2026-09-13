//! SubfrostPool — constant-product AMM pool (SUBFROST v0 port).
//!
//! LP token identity: `AssetId::of_contract(pool_contract_id)` — the pool
//! is its own LP token, exactly as upstream.
//!
//! Opcodes (upstream-compatible numbering):
//!   0    initialize (factory only, once)
//!   1    add_liquidity        [assets: token0 + token1]
//!   2    remove_liquidity     [assets: LP only]
//!   3    swap_exact_in        [assets: exactly one pool token]
//!   97   get_reserves
//!   98   cumulative price     -> deterministic Unsupported (no fake TWAP)
//!   99   get_name
//!   100  quote_exact_in
//!   999  pool_details
//!
//! Every mutating opcode runs under the storage reentrancy lock. Any error
//! reverts the entire call (storage, ledger, events) at the runtime layer.

#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

use alloc::vec::Vec;

use zalkanes_dex_core::encode::{
    self, decode_pool_add_liquidity_args, decode_pool_initialize_args, decode_pool_quote_args,
    decode_pool_remove_liquidity_args, decode_pool_swap_args, pool_op,
};
use zalkanes_dex_core::error::DexError;
use zalkanes_dex_core::events::DexEvent;
use zalkanes_dex_core::host::{get_u128, set_u128, Host};
use zalkanes_dex_core::math::{
    initial_liquidity, quote_add_liquidity, quote_remove_liquidity, quote_swap_exact_in,
};
use zalkanes_dex_core::types::{AssetId, ContractId, Holder, PoolDetails, SwapQuote};
use zalkanes_dex_core::v0::MINIMUM_LIQUIDITY;

const K_INIT: &[u8] = b"init";
const K_FACTORY: &[u8] = b"factory";
const K_TOKEN0: &[u8] = b"token0";
const K_TOKEN1: &[u8] = b"token1";
const K_RESERVE0: &[u8] = b"reserve0";
const K_RESERVE1: &[u8] = b"reserve1";
const K_LP_SUPPLY: &[u8] = b"lp_supply";
const K_PFEES0: &[u8] = b"pfees0";
const K_PFEES1: &[u8] = b"pfees1";
const K_LOCK: &[u8] = b"lock";
const K_NAME: &[u8] = b"name";

const FUEL_VIEW: u64 = 300;
const FUEL_INITIALIZE: u64 = 2_000;
const FUEL_ADD: u64 = 1_500;
const FUEL_REMOVE: u64 = 1_500;
const FUEL_SWAP: u64 = 1_200;

/// Contract entry, shared by the wasm binding and the native testkit.
pub fn dispatch<H: Host + ?Sized>(
    host: &mut H,
    opcode: u16,
    input: &[u8],
) -> Result<Vec<u8>, DexError> {
    match opcode {
        pool_op::INITIALIZE => {
            host.consume_fuel(FUEL_INITIALIZE)?;
            initialize(host, input)
        }
        pool_op::ADD_LIQUIDITY => {
            host.consume_fuel(FUEL_ADD)?;
            with_lock(host, input, add_liquidity)
        }
        pool_op::REMOVE_LIQUIDITY => {
            host.consume_fuel(FUEL_REMOVE)?;
            with_lock(host, input, remove_liquidity)
        }
        pool_op::SWAP_EXACT_IN => {
            host.consume_fuel(FUEL_SWAP)?;
            with_lock(host, input, swap_exact_in)
        }
        pool_op::GET_RESERVES => {
            host.consume_fuel(FUEL_VIEW)?;
            let s = state(host)?;
            Ok(encode::encode_reserves(s.reserve0, s.reserve1))
        }
        // No fake TWAP in v0: deterministic Unsupported (parity doc §deviation 5).
        pool_op::GET_PRICE_CUMULATIVE => Err(DexError::Unsupported),
        pool_op::GET_NAME => {
            host.consume_fuel(FUEL_VIEW)?;
            require_initialized(host)?;
            host.storage_get(K_NAME).ok_or(DexError::NotInitialized)
        }
        pool_op::QUOTE_EXACT_IN => {
            host.consume_fuel(FUEL_VIEW)?;
            quote(host, input)
        }
        pool_op::POOL_DETAILS => {
            host.consume_fuel(FUEL_VIEW)?;
            details(host)
        }
        _ => Err(DexError::InvalidOpcode),
    }
}

/// Reentrancy guard around every mutating operation. A failed call reverts
/// the lock write together with everything else; a nested mutating call
/// while the lock is held fails deterministically.
fn with_lock<H: Host + ?Sized>(
    host: &mut H,
    input: &[u8],
    f: fn(&mut H, &[u8]) -> Result<Vec<u8>, DexError>,
) -> Result<Vec<u8>, DexError> {
    if host.storage_get(K_LOCK).is_some() {
        return Err(DexError::Reentrancy);
    }
    host.storage_set(K_LOCK, &[1])?;
    let result = f(host, input)?;
    host.storage_delete(K_LOCK)?;
    Ok(result)
}

struct PoolStorage {
    factory: ContractId,
    token0: AssetId,
    token1: AssetId,
    reserve0: u128,
    reserve1: u128,
    lp_supply: u128,
    pfees0: u128,
    pfees1: u128,
}

fn require_initialized<H: Host + ?Sized>(host: &H) -> Result<(), DexError> {
    if host.storage_get(K_INIT).is_none() {
        return Err(DexError::NotInitialized);
    }
    Ok(())
}

fn get_id32<H: Host + ?Sized>(host: &H, key: &[u8]) -> Result<[u8; 32], DexError> {
    let bytes = host.storage_get(key).ok_or(DexError::NotInitialized)?;
    if bytes.len() != 32 {
        return Err(DexError::InvalidArguments);
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(&bytes);
    Ok(out)
}

fn state<H: Host + ?Sized>(host: &H) -> Result<PoolStorage, DexError> {
    require_initialized(host)?;
    Ok(PoolStorage {
        factory: ContractId(get_id32(host, K_FACTORY)?),
        token0: AssetId(get_id32(host, K_TOKEN0)?),
        token1: AssetId(get_id32(host, K_TOKEN1)?),
        reserve0: get_u128(host, K_RESERVE0)?,
        reserve1: get_u128(host, K_RESERVE1)?,
        lp_supply: get_u128(host, K_LP_SUPPLY)?,
        pfees0: get_u128(host, K_PFEES0)?,
        pfees1: get_u128(host, K_PFEES1)?,
    })
}

/// Expiry rule (upstream parity): 0 disables; otherwise the canonical
/// block height must not exceed it. The height always comes from the
/// deterministic execution context, never from the caller.
fn check_expiry<H: Host + ?Sized>(host: &H, expiry_height: u32) -> Result<(), DexError> {
    if expiry_height != 0 && host.block_height() > expiry_height {
        return Err(DexError::Expired);
    }
    Ok(())
}

/// The pool's own LP asset: identical bytes to the pool `ContractId`.
fn lp_asset<H: Host + ?Sized>(host: &H) -> AssetId {
    AssetId::of_contract(&host.self_id())
}

/// Incoming must be exactly `{token0: a0, token1: a1}`, both nonzero.
fn incoming_both<H: Host + ?Sized>(
    host: &H,
    token0: &AssetId,
    token1: &AssetId,
) -> Result<(u128, u128), DexError> {
    let incoming = host.incoming_assets();
    let mut a0 = 0u128;
    let mut a1 = 0u128;
    for (asset, amount) in &incoming {
        if asset == token0 {
            a0 = *amount;
        } else if asset == token1 {
            a1 = *amount;
        } else {
            return Err(DexError::InvalidAsset);
        }
    }
    if incoming.len() != 2 || a0 == 0 || a1 == 0 {
        return Err(DexError::InvalidIncomingAssets);
    }
    Ok((a0, a1))
}

fn initialize<H: Host + ?Sized>(host: &mut H, input: &[u8]) -> Result<Vec<u8>, DexError> {
    if host.storage_get(K_INIT).is_some() {
        return Err(DexError::AlreadyInitialized);
    }
    // Only the factory (a contract) that spawned this pool may initialize
    // it; external accounts cannot.
    let factory = match host.caller() {
        Holder::Contract(id) => id,
        Holder::External(_) => return Err(DexError::UnauthorizedInitialize),
    };
    let (token0, token1, provider) = decode_pool_initialize_args(input)?;
    if token0 == token1 {
        return Err(DexError::IdenticalAssets);
    }
    if token0 > token1 {
        // The factory must pass the canonical ordering.
        return Err(DexError::InvalidArguments);
    }
    let own = lp_asset(host);
    if token0 == own || token1 == own {
        return Err(DexError::InvalidAsset);
    }
    let (amount0, amount1) = incoming_both(host, &token0, &token1)?;
    let (provider_lp, gross_lp) = initial_liquidity(amount0, amount1)?;

    // Write the complete initialized state AND take the reentrancy lock
    // BEFORE any outbound call (name resolution calls the token
    // contracts): a malicious token calling back in hits
    // AlreadyInitialized / Reentrancy, never a half-initialized pool.
    host.storage_set(K_FACTORY, &factory.0)?;
    host.storage_set(K_TOKEN0, &token0.0)?;
    host.storage_set(K_TOKEN1, &token1.0)?;
    set_u128(host, K_RESERVE0, amount0)?;
    set_u128(host, K_RESERVE1, amount1)?;
    // Locked MINIMUM_LIQUIDITY: counted in supply, held by nobody.
    set_u128(host, K_LP_SUPPLY, gross_lp)?;
    set_u128(host, K_PFEES0, 0)?;
    set_u128(host, K_PFEES1, 0)?;
    host.storage_set(K_INIT, &[1])?;
    host.storage_set(K_LOCK, &[1])?;
    let name = pool_name(host, &token0, &token1);
    host.storage_set(K_NAME, &name)?;
    host.storage_delete(K_LOCK)?;

    host.mint_own_asset(&provider, provider_lp)?;

    let mut out = Vec::with_capacity(32);
    out.extend_from_slice(&provider_lp.to_be_bytes());
    out.extend_from_slice(&MINIMUM_LIQUIDITY.to_be_bytes());
    Ok(out)
}

/// "{name0} / {name1} LP", falling back to a short asset-id prefix when a
/// constituent has no readable name (upstream falls back to "{block},{tx}").
fn pool_name<H: Host + ?Sized>(host: &mut H, token0: &AssetId, token1: &AssetId) -> Vec<u8> {
    let mut out = Vec::with_capacity(48);
    push_token_name(host, token0, &mut out);
    out.extend_from_slice(b" / ");
    push_token_name(host, token1, &mut out);
    out.extend_from_slice(b" LP");
    out
}

fn push_token_name<H: Host + ?Sized>(host: &mut H, token: &AssetId, out: &mut Vec<u8>) {
    match host.call(&ContractId(token.0), encode::token_op::GET_NAME, &[], &[]) {
        Ok(name) if !name.is_empty() && core::str::from_utf8(&name).is_ok() => {
            out.extend_from_slice(&name);
        }
        _ => {
            const HEX: &[u8; 16] = b"0123456789abcdef";
            for byte in token.0.iter().take(4) {
                out.push(HEX[usize::from(byte >> 4)]);
                out.push(HEX[usize::from(byte & 0x0f)]);
            }
        }
    }
}

fn add_liquidity<H: Host + ?Sized>(host: &mut H, input: &[u8]) -> Result<Vec<u8>, DexError> {
    let s = state(host)?;
    let (min_lp_out, expiry) = decode_pool_add_liquidity_args(input)?;
    check_expiry(host, expiry)?;
    let (desired0, desired1) = incoming_both(host, &s.token0, &s.token1)?;

    let outcome = quote_add_liquidity(s.reserve0, s.reserve1, s.lp_supply, desired0, desired1)?;
    if outcome.lp_minted < min_lp_out {
        return Err(DexError::SlippageExceeded);
    }

    let caller = host.caller();
    if outcome.refund0 > 0 {
        host.transfer_out(&caller, &s.token0, outcome.refund0)?;
    }
    if outcome.refund1 > 0 {
        host.transfer_out(&caller, &s.token1, outcome.refund1)?;
    }

    set_u128(
        host,
        K_RESERVE0,
        s.reserve0
            .checked_add(outcome.accepted0)
            .ok_or(DexError::ArithmeticOverflow)?,
    )?;
    set_u128(
        host,
        K_RESERVE1,
        s.reserve1
            .checked_add(outcome.accepted1)
            .ok_or(DexError::ArithmeticOverflow)?,
    )?;
    set_u128(
        host,
        K_LP_SUPPLY,
        s.lp_supply
            .checked_add(outcome.lp_minted)
            .ok_or(DexError::ArithmeticOverflow)?,
    )?;
    host.mint_own_asset(&caller, outcome.lp_minted)?;

    host.emit_event(&DexEvent::LiquidityAdded {
        pool: host.self_id(),
        provider: caller,
        amount0: outcome.accepted0,
        amount1: outcome.accepted1,
        lp_minted: outcome.lp_minted,
    });

    let mut out = Vec::with_capacity(80);
    out.extend_from_slice(&outcome.accepted0.to_be_bytes());
    out.extend_from_slice(&outcome.accepted1.to_be_bytes());
    out.extend_from_slice(&outcome.refund0.to_be_bytes());
    out.extend_from_slice(&outcome.refund1.to_be_bytes());
    out.extend_from_slice(&outcome.lp_minted.to_be_bytes());
    Ok(out)
}

fn remove_liquidity<H: Host + ?Sized>(host: &mut H, input: &[u8]) -> Result<Vec<u8>, DexError> {
    let s = state(host)?;
    let (min0, min1, expiry) = decode_pool_remove_liquidity_args(input)?;
    check_expiry(host, expiry)?;

    let incoming = host.incoming_assets();
    let own = lp_asset(host);
    let lp_amount = match incoming.as_slice() {
        [(asset, amount)] if *asset == own => *amount,
        [] => return Err(DexError::InvalidIncomingAssets),
        _ => return Err(DexError::InvalidAsset),
    };
    if lp_amount == 0 {
        return Err(DexError::ZeroAmount);
    }

    let (amount0, amount1) =
        quote_remove_liquidity(s.reserve0, s.reserve1, s.lp_supply, lp_amount)?;
    if amount0 < min0 || amount1 < min1 {
        return Err(DexError::SlippageExceeded);
    }
    // The permanently locked minimum can never be redeemed: no holder owns
    // it, so circulating LP is always <= supply - MINIMUM_LIQUIDITY.
    let remaining_supply = s
        .lp_supply
        .checked_sub(lp_amount)
        .ok_or(DexError::InsufficientLp)?;
    if remaining_supply < MINIMUM_LIQUIDITY {
        return Err(DexError::InsufficientLp);
    }

    host.burn_own_asset(lp_amount)?;
    set_u128(
        host,
        K_RESERVE0,
        s.reserve0
            .checked_sub(amount0)
            .ok_or(DexError::ArithmeticUnderflow)?,
    )?;
    set_u128(
        host,
        K_RESERVE1,
        s.reserve1
            .checked_sub(amount1)
            .ok_or(DexError::ArithmeticUnderflow)?,
    )?;
    set_u128(host, K_LP_SUPPLY, remaining_supply)?;

    let caller = host.caller();
    host.transfer_out(&caller, &s.token0, amount0)?;
    host.transfer_out(&caller, &s.token1, amount1)?;

    host.emit_event(&DexEvent::LiquidityRemoved {
        pool: host.self_id(),
        provider: caller,
        amount0,
        amount1,
        lp_burned: lp_amount,
    });

    let mut out = Vec::with_capacity(32);
    out.extend_from_slice(&amount0.to_be_bytes());
    out.extend_from_slice(&amount1.to_be_bytes());
    Ok(out)
}

fn swap_exact_in<H: Host + ?Sized>(host: &mut H, input: &[u8]) -> Result<Vec<u8>, DexError> {
    let s = state(host)?;
    let (min_amount_out, expiry) = decode_pool_swap_args(input)?;
    check_expiry(host, expiry)?;

    let incoming = host.incoming_assets();
    let (token_in, amount_in) = match incoming.as_slice() {
        [(asset, amount)] => (*asset, *amount),
        [] => return Err(DexError::InvalidIncomingAssets),
        _ => {
            // More than one incoming asset: both pool tokens is a
            // multiplicity error; anything else is an unknown asset.
            if incoming
                .iter()
                .all(|(a, _)| *a == s.token0 || *a == s.token1)
            {
                return Err(DexError::InvalidIncomingAssets);
            }
            return Err(DexError::InvalidAsset);
        }
    };
    if amount_in == 0 {
        return Err(DexError::ZeroAmount);
    }

    let zero_for_one = if token_in == s.token0 {
        true
    } else if token_in == s.token1 {
        false
    } else {
        return Err(DexError::InvalidAsset);
    };
    let (reserve_in, reserve_out, token_out) = if zero_for_one {
        (s.reserve0, s.reserve1, s.token1)
    } else {
        (s.reserve1, s.reserve0, s.token0)
    };

    let outcome = quote_swap_exact_in(reserve_in, reserve_out, amount_in)?;
    if outcome.amount_out < min_amount_out {
        return Err(DexError::SlippageExceeded);
    }

    // Reserve/fee accounting: the LP share of the fee stays in reserves;
    // the protocol share is excluded from reserves and accumulated.
    let new_reserve_in = reserve_in
        .checked_add(amount_in)
        .and_then(|r| r.checked_sub(outcome.fees.protocol_fee))
        .ok_or(DexError::ArithmeticOverflow)?;
    let new_reserve_out = reserve_out
        .checked_sub(outcome.amount_out)
        .ok_or(DexError::ArithmeticUnderflow)?;

    if zero_for_one {
        set_u128(host, K_RESERVE0, new_reserve_in)?;
        set_u128(host, K_RESERVE1, new_reserve_out)?;
        set_u128(
            host,
            K_PFEES0,
            s.pfees0
                .checked_add(outcome.fees.protocol_fee)
                .ok_or(DexError::ArithmeticOverflow)?,
        )?;
    } else {
        set_u128(host, K_RESERVE1, new_reserve_in)?;
        set_u128(host, K_RESERVE0, new_reserve_out)?;
        set_u128(
            host,
            K_PFEES1,
            s.pfees1
                .checked_add(outcome.fees.protocol_fee)
                .ok_or(DexError::ArithmeticOverflow)?,
        )?;
    }

    let caller = host.caller();
    host.transfer_out(&caller, &token_out, outcome.amount_out)?;

    host.emit_event(&DexEvent::Swap {
        pool: host.self_id(),
        token_in,
        token_out,
        amount_in,
        amount_out: outcome.amount_out,
        total_fee: outcome.fees.total_fee,
        lp_fee: outcome.fees.lp_fee,
        protocol_fee: outcome.fees.protocol_fee,
    });

    let mut out = Vec::with_capacity(64);
    out.extend_from_slice(&outcome.amount_out.to_be_bytes());
    out.extend_from_slice(&outcome.fees.total_fee.to_be_bytes());
    out.extend_from_slice(&outcome.fees.lp_fee.to_be_bytes());
    out.extend_from_slice(&outcome.fees.protocol_fee.to_be_bytes());
    Ok(out)
}

fn quote<H: Host + ?Sized>(host: &H, input: &[u8]) -> Result<Vec<u8>, DexError> {
    let s = state(host)?;
    let (token_in, amount_in) = decode_pool_quote_args(input)?;
    let (reserve_in, reserve_out, token_out) = if token_in == s.token0 {
        (s.reserve0, s.reserve1, s.token1)
    } else if token_in == s.token1 {
        (s.reserve1, s.reserve0, s.token0)
    } else {
        return Err(DexError::InvalidAsset);
    };
    let outcome = quote_swap_exact_in(reserve_in, reserve_out, amount_in)?;
    Ok(encode::encode_swap_quote(&SwapQuote {
        token_in,
        token_out,
        amount_in,
        amount_out: outcome.amount_out,
        total_fee: outcome.fees.total_fee,
        lp_fee: outcome.fees.lp_fee,
        protocol_fee: outcome.fees.protocol_fee,
    }))
}

fn details<H: Host + ?Sized>(host: &H) -> Result<Vec<u8>, DexError> {
    let s = state(host)?;
    Ok(encode::encode_pool_details(&PoolDetails {
        pool_id: host.self_id(),
        factory_id: s.factory,
        token0: s.token0,
        token1: s.token1,
        reserve0: s.reserve0,
        reserve1: s.reserve1,
        total_lp_supply: s.lp_supply,
        protocol_fees0: s.pfees0,
        protocol_fees1: s.pfees1,
    }))
}

zalkanes_dex_wasm::contract_entry!(crate::dispatch);
