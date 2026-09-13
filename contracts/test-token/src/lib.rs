//! Deterministic test-only token contract for DEX testing.
//!
//! NOT a production token: `mint_for_test` is deliberately permissionless
//! so fixtures can fund any holder. Never deploy outside test networks.
//! Fixture names are `Zalkanes Test Asset A/B/C` — never ZEC or any
//! wrapped-ZEC naming.
//!
//! Opcodes:
//!   0   initialize(name_len u16 BE ‖ name utf8)   — once
//!   1   mint_for_test(to: Holder(33) ‖ amount u128 BE)
//!   99  get_name() -> name bytes

#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

use alloc::vec::Vec;

use zalkanes_dex_core::encode::{decode_token_initialize_args, decode_token_mint_args, token_op};
use zalkanes_dex_core::error::DexError;
use zalkanes_dex_core::host::{get_u128, set_u128, Host};

const K_INIT: &[u8] = b"init";
const K_NAME: &[u8] = b"name";
const K_SUPPLY: &[u8] = b"supply";

const FUEL_BASE: u64 = 300;

/// Contract entry, shared by the wasm binding and the native testkit.
pub fn dispatch<H: Host + ?Sized>(
    host: &mut H,
    opcode: u16,
    input: &[u8],
) -> Result<Vec<u8>, DexError> {
    host.consume_fuel(FUEL_BASE)?;
    match opcode {
        token_op::INITIALIZE => initialize(host, input),
        token_op::MINT_FOR_TEST => mint_for_test(host, input),
        token_op::GET_NAME => get_name(host),
        _ => Err(DexError::InvalidOpcode),
    }
}

fn require_initialized<H: Host + ?Sized>(host: &H) -> Result<(), DexError> {
    if host.storage_get(K_INIT).is_none() {
        return Err(DexError::NotInitialized);
    }
    Ok(())
}

fn initialize<H: Host + ?Sized>(host: &mut H, input: &[u8]) -> Result<Vec<u8>, DexError> {
    if host.storage_get(K_INIT).is_some() {
        return Err(DexError::AlreadyInitialized);
    }
    if !host.incoming_assets().is_empty() {
        return Err(DexError::InvalidIncomingAssets);
    }
    let name = decode_token_initialize_args(input)?;
    host.storage_set(K_NAME, &name)?;
    host.storage_set(K_INIT, &[1])?;
    Ok(Vec::new())
}

fn mint_for_test<H: Host + ?Sized>(host: &mut H, input: &[u8]) -> Result<Vec<u8>, DexError> {
    require_initialized(host)?;
    if !host.incoming_assets().is_empty() {
        return Err(DexError::InvalidIncomingAssets);
    }
    let (to, amount) = decode_token_mint_args(input)?;
    if amount == 0 {
        return Err(DexError::ZeroAmount);
    }
    let supply = get_u128(host, K_SUPPLY)?;
    let new_supply = supply
        .checked_add(amount)
        .ok_or(DexError::ArithmeticOverflow)?;
    host.mint_own_asset(&to, amount)?;
    set_u128(host, K_SUPPLY, new_supply)?;
    Ok(Vec::new())
}

fn get_name<H: Host + ?Sized>(host: &H) -> Result<Vec<u8>, DexError> {
    require_initialized(host)?;
    host.storage_get(K_NAME).ok_or(DexError::NotInitialized)
}

zalkanes_dex_wasm::contract_entry!(crate::dispatch);
