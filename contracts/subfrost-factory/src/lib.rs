//! SubfrostFactory — permissionless AMM pool factory (SUBFROST v0 port).
//!
//! No owner, no admin, no token allowlist. Anyone may create a pool for a
//! distinct asset pair; the canonical A/B == B/A pair maps to exactly one
//! pool. Registration happens only after the spawned pool initializes
//! successfully; any failure reverts the whole creation atomically.
//!
//! Opcodes:
//!   0  initialize(pool_template_hash 32)      — once, fixes the template
//!   1  create_pool(tokenA 32 ‖ tokenB 32)     [assets: initial amounts]
//!   2  get_pool(tokenA ‖ tokenB) -> pool_id | empty
//!   3  get_all_pools([start u32 ‖ limit u32]) -> count ‖ ids (creation order)
//!   4  pool_count() -> u128

#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

use alloc::vec::Vec;

use zalkanes_dex_core::encode::{
    decode_factory_create_pool_args, decode_factory_initialize_args, encode_pool_list, factory_op,
    Reader,
};
use zalkanes_dex_core::error::DexError;
use zalkanes_dex_core::events::DexEvent;
use zalkanes_dex_core::host::{get_u128, set_u128, ContractSpawner, Host, PoolInit};
use zalkanes_dex_core::types::{ContractId, Pair};

const K_INIT: &[u8] = b"init";
const K_TEMPLATE: &[u8] = b"template";
const K_COUNT: &[u8] = b"count";
const P_PAIR: &[u8] = b"pair/";
const P_POOL_INDEX: &[u8] = b"pool/";
const P_POOL_PAIR: &[u8] = b"poolpair/";

const FUEL_VIEW: u64 = 300;
const FUEL_INITIALIZE: u64 = 800;
const FUEL_CREATE: u64 = 3_000;

/// Registry size bound: keeps enumeration and pagination in u32 space.
const MAX_POOLS: u128 = u32::MAX as u128;

/// Contract entry, shared by the wasm binding and the native testkit.
pub fn dispatch<H: Host + ?Sized>(
    host: &mut H,
    opcode: u16,
    input: &[u8],
) -> Result<Vec<u8>, DexError> {
    match opcode {
        factory_op::INITIALIZE => {
            host.consume_fuel(FUEL_INITIALIZE)?;
            initialize(host, input)
        }
        factory_op::CREATE_POOL => {
            host.consume_fuel(FUEL_CREATE)?;
            create_pool(host, input)
        }
        factory_op::GET_POOL => {
            host.consume_fuel(FUEL_VIEW)?;
            get_pool(host, input)
        }
        factory_op::GET_ALL_POOLS => {
            host.consume_fuel(FUEL_VIEW)?;
            get_all_pools(host, input)
        }
        factory_op::POOL_COUNT => {
            host.consume_fuel(FUEL_VIEW)?;
            require_initialized(host)?;
            Ok(get_u128(host, K_COUNT)?.to_be_bytes().to_vec())
        }
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
    let template = decode_factory_initialize_args(input)?;
    host.storage_set(K_TEMPLATE, &template)?;
    set_u128(host, K_COUNT, 0)?;
    host.storage_set(K_INIT, &[1])?;
    Ok(Vec::new())
}

fn pair_storage_key(pair: &Pair) -> Vec<u8> {
    let mut key = Vec::with_capacity(P_PAIR.len() + 89);
    key.extend_from_slice(P_PAIR);
    key.extend_from_slice(&pair.key());
    key
}

fn index_storage_key(index: u32) -> Vec<u8> {
    let mut key = Vec::with_capacity(P_POOL_INDEX.len() + 4);
    key.extend_from_slice(P_POOL_INDEX);
    key.extend_from_slice(&index.to_be_bytes());
    key
}

fn create_pool<H: Host + ?Sized>(host: &mut H, input: &[u8]) -> Result<Vec<u8>, DexError> {
    require_initialized(host)?;
    let (token_a, token_b) = decode_factory_create_pool_args(input)?;
    let pair = Pair::canonical(token_a, token_b)?;

    let pair_key = pair_storage_key(&pair);
    if host.storage_get(&pair_key).is_some() {
        return Err(DexError::DuplicatePool);
    }

    // Map the attached initial amounts onto the canonical order.
    let incoming = host.incoming_assets();
    let mut amount0 = 0u128;
    let mut amount1 = 0u128;
    for (asset, amount) in &incoming {
        if *asset == pair.token0 {
            amount0 = *amount;
        } else if *asset == pair.token1 {
            amount1 = *amount;
        } else {
            return Err(DexError::InvalidAsset);
        }
    }
    if incoming.len() != 2 || amount0 == 0 || amount1 == 0 {
        return Err(DexError::InvalidIncomingAssets);
    }

    let count = get_u128(host, K_COUNT)?;
    if count >= MAX_POOLS {
        return Err(DexError::ArithmeticOverflow);
    }

    let template_bytes = host
        .storage_get(K_TEMPLATE)
        .ok_or(DexError::NotInitialized)?;
    if template_bytes.len() != 32 {
        return Err(DexError::InvalidArguments);
    }
    let mut template = [0u8; 32];
    template.copy_from_slice(&template_bytes);

    // Spawn + initialize atomically. Any failure (spawn error, pool init
    // error, insufficient initial liquidity, ...) propagates and reverts
    // this whole call — the registry is only written after success.
    let provider = host.caller();
    let pool_id = host.spawn_pool(
        &template,
        PoolInit {
            token0: pair.token0,
            token1: pair.token1,
            amount0,
            amount1,
            provider,
        },
    )?;

    // Registration (only after successful initialization).
    host.storage_set(&pair_key, &pool_id.0)?;
    // count < MAX_POOLS <= u32::MAX, so the cast is lossless.
    host.storage_set(&index_storage_key(count as u32), &pool_id.0)?;
    let mut pool_pair_key = Vec::with_capacity(P_POOL_PAIR.len() + 32);
    pool_pair_key.extend_from_slice(P_POOL_PAIR);
    pool_pair_key.extend_from_slice(&pool_id.0);
    let mut pair_bytes = Vec::with_capacity(64);
    pair_bytes.extend_from_slice(&pair.token0.0);
    pair_bytes.extend_from_slice(&pair.token1.0);
    host.storage_set(&pool_pair_key, &pair_bytes)?;
    set_u128(
        host,
        K_COUNT,
        count.checked_add(1).ok_or(DexError::ArithmeticOverflow)?,
    )?;

    // Event field `lp_minted` = LP actually minted to the provider
    // (initial supply minus the permanently locked minimum).
    let lp_minted = {
        let details = host.call(
            &pool_id,
            zalkanes_dex_core::encode::pool_op::POOL_DETAILS,
            &[],
            &[],
        )?;
        zalkanes_dex_core::encode::decode_pool_details(&details)?
            .total_lp_supply
            .checked_sub(zalkanes_dex_core::v0::MINIMUM_LIQUIDITY)
            .ok_or(DexError::ArithmeticUnderflow)?
    };

    host.emit_event(&DexEvent::PoolCreated {
        factory: host.self_id(),
        pool: pool_id,
        token0: pair.token0,
        token1: pair.token1,
        amount0,
        amount1,
        lp_minted,
    });

    Ok(pool_id.0.to_vec())
}

fn get_pool<H: Host + ?Sized>(host: &H, input: &[u8]) -> Result<Vec<u8>, DexError> {
    require_initialized(host)?;
    let (token_a, token_b) = decode_factory_create_pool_args(input)?;
    let pair = Pair::canonical(token_a, token_b)?;
    match host.storage_get(&pair_storage_key(&pair)) {
        Some(pool_id) if pool_id.len() == 32 => Ok(pool_id),
        Some(_) => Err(DexError::InvalidArguments),
        None => Ok(Vec::new()),
    }
}

fn get_all_pools<H: Host + ?Sized>(host: &H, input: &[u8]) -> Result<Vec<u8>, DexError> {
    require_initialized(host)?;
    let count_u128 = get_u128(host, K_COUNT)?;
    // count is always < MAX_POOLS (u32 space) by construction.
    let count = count_u128 as u32;
    let (start, limit) = if input.is_empty() {
        (0u32, count)
    } else {
        let mut r = Reader::new(input);
        let start = r.u32_be()?;
        let limit = r.u32_be()?;
        r.finish()?;
        (start, limit)
    };
    let end = start.saturating_add(limit).min(count);
    let mut pools = Vec::new();
    let mut index = start;
    while index < end {
        let bytes = host
            .storage_get(&index_storage_key(index))
            .ok_or(DexError::InvalidArguments)?;
        if bytes.len() != 32 {
            return Err(DexError::InvalidArguments);
        }
        let mut id = [0u8; 32];
        id.copy_from_slice(&bytes);
        pools.push(ContractId(id));
        index += 1;
    }
    Ok(encode_pool_list(&pools))
}

zalkanes_dex_wasm::contract_entry!(crate::dispatch);
