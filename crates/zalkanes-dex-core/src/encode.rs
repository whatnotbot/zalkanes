//! Deterministic byte encodings for calldata, view results and quotes.
//!
//! Platform convention: all integers are big-endian, fixed width.
//! Malformed input always decodes to `DexError::InvalidArguments`;
//! nothing here panics on adversarial bytes.

use alloc::vec::Vec;

use crate::error::DexError;
use crate::types::{AssetId, ContractId, Holder, PoolDetails, SwapQuote};

/// Safe sequential reader over calldata.
pub struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    #[must_use]
    pub fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }

    pub fn take(&mut self, n: usize) -> Result<&'a [u8], DexError> {
        let end = self.pos.checked_add(n).ok_or(DexError::InvalidArguments)?;
        if end > self.bytes.len() {
            return Err(DexError::InvalidArguments);
        }
        let out = &self.bytes[self.pos..end];
        self.pos = end;
        Ok(out)
    }

    pub fn u128_be(&mut self) -> Result<u128, DexError> {
        let mut buf = [0u8; 16];
        buf.copy_from_slice(self.take(16)?);
        Ok(u128::from_be_bytes(buf))
    }

    pub fn u32_be(&mut self) -> Result<u32, DexError> {
        let mut buf = [0u8; 4];
        buf.copy_from_slice(self.take(4)?);
        Ok(u32::from_be_bytes(buf))
    }

    pub fn id32(&mut self) -> Result<[u8; 32], DexError> {
        let mut buf = [0u8; 32];
        buf.copy_from_slice(self.take(32)?);
        Ok(buf)
    }

    pub fn asset(&mut self) -> Result<AssetId, DexError> {
        Ok(AssetId(self.id32()?))
    }

    pub fn contract(&mut self) -> Result<ContractId, DexError> {
        Ok(ContractId(self.id32()?))
    }

    pub fn holder(&mut self) -> Result<Holder, DexError> {
        Holder::from_bytes(self.take(33)?)
    }

    /// The input must be fully consumed; trailing bytes are malformed.
    pub fn finish(self) -> Result<(), DexError> {
        if self.pos == self.bytes.len() {
            Ok(())
        } else {
            Err(DexError::InvalidArguments)
        }
    }
}

// ── pool opcodes ─────────────────────────────────────────────────────────

/// Pool opcode numbers (v0 ABI, upstream-compatible where the set overlaps).
pub mod pool_op {
    pub const INITIALIZE: u16 = 0;
    pub const ADD_LIQUIDITY: u16 = 1;
    pub const REMOVE_LIQUIDITY: u16 = 2;
    pub const SWAP_EXACT_IN: u16 = 3;
    pub const GET_RESERVES: u16 = 97;
    pub const GET_PRICE_CUMULATIVE: u16 = 98;
    pub const GET_NAME: u16 = 99;
    pub const QUOTE_EXACT_IN: u16 = 100;
    pub const POOL_DETAILS: u16 = 999;
}

/// Factory opcode numbers (v0 ABI).
pub mod factory_op {
    pub const INITIALIZE: u16 = 0;
    pub const CREATE_POOL: u16 = 1;
    pub const GET_POOL: u16 = 2;
    pub const GET_ALL_POOLS: u16 = 3;
    pub const POOL_COUNT: u16 = 4;
}

/// Test-token opcode numbers.
pub mod token_op {
    pub const INITIALIZE: u16 = 0;
    pub const MINT_FOR_TEST: u16 = 1;
    pub const GET_NAME: u16 = 99;
}

/// Pool `initialize` calldata: token0 ‖ token1 ‖ provider(33).
#[must_use]
pub fn pool_initialize_args(token0: &AssetId, token1: &AssetId, provider: &Holder) -> Vec<u8> {
    let mut out = Vec::with_capacity(97);
    out.extend_from_slice(&token0.0);
    out.extend_from_slice(&token1.0);
    out.extend_from_slice(&provider.to_bytes());
    out
}

pub fn decode_pool_initialize_args(input: &[u8]) -> Result<(AssetId, AssetId, Holder), DexError> {
    let mut r = Reader::new(input);
    let token0 = r.asset()?;
    let token1 = r.asset()?;
    let provider = r.holder()?;
    r.finish()?;
    Ok((token0, token1, provider))
}

/// Pool `add_liquidity` calldata: min_lp_out(16) ‖ expiry_height(4).
#[must_use]
pub fn pool_add_liquidity_args(min_lp_out: u128, expiry_height: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity(20);
    out.extend_from_slice(&min_lp_out.to_be_bytes());
    out.extend_from_slice(&expiry_height.to_be_bytes());
    out
}

pub fn decode_pool_add_liquidity_args(input: &[u8]) -> Result<(u128, u32), DexError> {
    let mut r = Reader::new(input);
    let min_lp_out = r.u128_be()?;
    let expiry = r.u32_be()?;
    r.finish()?;
    Ok((min_lp_out, expiry))
}

/// Pool `remove_liquidity` calldata: min0(16) ‖ min1(16) ‖ expiry(4).
#[must_use]
pub fn pool_remove_liquidity_args(min0: u128, min1: u128, expiry_height: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity(36);
    out.extend_from_slice(&min0.to_be_bytes());
    out.extend_from_slice(&min1.to_be_bytes());
    out.extend_from_slice(&expiry_height.to_be_bytes());
    out
}

pub fn decode_pool_remove_liquidity_args(input: &[u8]) -> Result<(u128, u128, u32), DexError> {
    let mut r = Reader::new(input);
    let min0 = r.u128_be()?;
    let min1 = r.u128_be()?;
    let expiry = r.u32_be()?;
    r.finish()?;
    Ok((min0, min1, expiry))
}

/// Pool `swap_exact_in` calldata: min_amount_out(16) ‖ expiry(4).
#[must_use]
pub fn pool_swap_args(min_amount_out: u128, expiry_height: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity(20);
    out.extend_from_slice(&min_amount_out.to_be_bytes());
    out.extend_from_slice(&expiry_height.to_be_bytes());
    out
}

pub fn decode_pool_swap_args(input: &[u8]) -> Result<(u128, u32), DexError> {
    let mut r = Reader::new(input);
    let min_out = r.u128_be()?;
    let expiry = r.u32_be()?;
    r.finish()?;
    Ok((min_out, expiry))
}

/// Pool `quote_exact_in` view calldata: token_in(32) ‖ amount_in(16).
#[must_use]
pub fn pool_quote_args(token_in: &AssetId, amount_in: u128) -> Vec<u8> {
    let mut out = Vec::with_capacity(48);
    out.extend_from_slice(&token_in.0);
    out.extend_from_slice(&amount_in.to_be_bytes());
    out
}

pub fn decode_pool_quote_args(input: &[u8]) -> Result<(AssetId, u128), DexError> {
    let mut r = Reader::new(input);
    let token_in = r.asset()?;
    let amount_in = r.u128_be()?;
    r.finish()?;
    Ok((token_in, amount_in))
}

// ── view result encodings ────────────────────────────────────────────────

/// `get_reserves` result: reserve0(16) ‖ reserve1(16).
#[must_use]
pub fn encode_reserves(reserve0: u128, reserve1: u128) -> Vec<u8> {
    let mut out = Vec::with_capacity(32);
    out.extend_from_slice(&reserve0.to_be_bytes());
    out.extend_from_slice(&reserve1.to_be_bytes());
    out
}

pub fn decode_reserves(bytes: &[u8]) -> Result<(u128, u128), DexError> {
    let mut r = Reader::new(bytes);
    let r0 = r.u128_be()?;
    let r1 = r.u128_be()?;
    r.finish()?;
    Ok((r0, r1))
}

/// `pool_details` result: pool ‖ factory ‖ token0 ‖ token1 ‖ five u128 fields.
#[must_use]
pub fn encode_pool_details(d: &PoolDetails) -> Vec<u8> {
    let mut out = Vec::with_capacity(208);
    out.extend_from_slice(&d.pool_id.0);
    out.extend_from_slice(&d.factory_id.0);
    out.extend_from_slice(&d.token0.0);
    out.extend_from_slice(&d.token1.0);
    out.extend_from_slice(&d.reserve0.to_be_bytes());
    out.extend_from_slice(&d.reserve1.to_be_bytes());
    out.extend_from_slice(&d.total_lp_supply.to_be_bytes());
    out.extend_from_slice(&d.protocol_fees0.to_be_bytes());
    out.extend_from_slice(&d.protocol_fees1.to_be_bytes());
    out
}

pub fn decode_pool_details(bytes: &[u8]) -> Result<PoolDetails, DexError> {
    let mut r = Reader::new(bytes);
    let details = PoolDetails {
        pool_id: r.contract()?,
        factory_id: r.contract()?,
        token0: r.asset()?,
        token1: r.asset()?,
        reserve0: r.u128_be()?,
        reserve1: r.u128_be()?,
        total_lp_supply: r.u128_be()?,
        protocol_fees0: r.u128_be()?,
        protocol_fees1: r.u128_be()?,
    };
    r.finish()?;
    Ok(details)
}

/// `quote_exact_in` result encoding.
#[must_use]
pub fn encode_swap_quote(q: &SwapQuote) -> Vec<u8> {
    let mut out = Vec::with_capacity(144);
    out.extend_from_slice(&q.token_in.0);
    out.extend_from_slice(&q.token_out.0);
    out.extend_from_slice(&q.amount_in.to_be_bytes());
    out.extend_from_slice(&q.amount_out.to_be_bytes());
    out.extend_from_slice(&q.total_fee.to_be_bytes());
    out.extend_from_slice(&q.lp_fee.to_be_bytes());
    out.extend_from_slice(&q.protocol_fee.to_be_bytes());
    out
}

pub fn decode_swap_quote(bytes: &[u8]) -> Result<SwapQuote, DexError> {
    let mut r = Reader::new(bytes);
    let quote = SwapQuote {
        token_in: r.asset()?,
        token_out: r.asset()?,
        amount_in: r.u128_be()?,
        amount_out: r.u128_be()?,
        total_fee: r.u128_be()?,
        lp_fee: r.u128_be()?,
        protocol_fee: r.u128_be()?,
    };
    r.finish()?;
    Ok(quote)
}

// ── factory calldata ─────────────────────────────────────────────────────

/// Factory `initialize` calldata: pool_template(32).
#[must_use]
pub fn factory_initialize_args(pool_template: &[u8; 32]) -> Vec<u8> {
    pool_template.to_vec()
}

pub fn decode_factory_initialize_args(input: &[u8]) -> Result<[u8; 32], DexError> {
    let mut r = Reader::new(input);
    let template = r.id32()?;
    r.finish()?;
    Ok(template)
}

/// Factory `create_pool` calldata: tokenA(32) ‖ tokenB(32).
/// The initial amounts are the assets attached to the call.
#[must_use]
pub fn factory_create_pool_args(token_a: &AssetId, token_b: &AssetId) -> Vec<u8> {
    let mut out = Vec::with_capacity(64);
    out.extend_from_slice(&token_a.0);
    out.extend_from_slice(&token_b.0);
    out
}

pub fn decode_factory_create_pool_args(input: &[u8]) -> Result<(AssetId, AssetId), DexError> {
    let mut r = Reader::new(input);
    let a = r.asset()?;
    let b = r.asset()?;
    r.finish()?;
    Ok((a, b))
}

/// Factory `get_pool` calldata: tokenA(32) ‖ tokenB(32).
#[must_use]
pub fn factory_get_pool_args(token_a: &AssetId, token_b: &AssetId) -> Vec<u8> {
    factory_create_pool_args(token_a, token_b)
}

/// `get_all_pools` result: count(4) ‖ count × pool_id(32), creation order.
#[must_use]
pub fn encode_pool_list(pools: &[ContractId]) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + pools.len() * 32);
    // Bounded by MAX_POOLS_PER_FACTORY (u32 range enforced at creation).
    out.extend_from_slice(&(pools.len() as u32).to_be_bytes());
    for p in pools {
        out.extend_from_slice(&p.0);
    }
    out
}

pub fn decode_pool_list(bytes: &[u8]) -> Result<Vec<ContractId>, DexError> {
    let mut r = Reader::new(bytes);
    let count = r.u32_be()? as usize;
    let mut pools = Vec::with_capacity(count.min(1024));
    for _ in 0..count {
        pools.push(r.contract()?);
    }
    r.finish()?;
    Ok(pools)
}

// ── test-token calldata ──────────────────────────────────────────────────

/// Token `initialize` calldata: name_len(2 BE) ‖ name bytes (UTF-8).
#[must_use]
pub fn token_initialize_args(name: &str) -> Vec<u8> {
    let bytes = name.as_bytes();
    let mut out = Vec::with_capacity(2 + bytes.len());
    out.extend_from_slice(&(bytes.len() as u16).to_be_bytes());
    out.extend_from_slice(bytes);
    out
}

pub fn decode_token_initialize_args(input: &[u8]) -> Result<Vec<u8>, DexError> {
    let mut r = Reader::new(input);
    let len_bytes = r.take(2)?;
    let len = u16::from_be_bytes([len_bytes[0], len_bytes[1]]) as usize;
    let name = r.take(len)?.to_vec();
    r.finish()?;
    if name.is_empty() || core::str::from_utf8(&name).is_err() {
        return Err(DexError::InvalidArguments);
    }
    Ok(name)
}

/// Token `mint_for_test` calldata: to(33) ‖ amount(16).
#[must_use]
pub fn token_mint_args(to: &Holder, amount: u128) -> Vec<u8> {
    let mut out = Vec::with_capacity(49);
    out.extend_from_slice(&to.to_bytes());
    out.extend_from_slice(&amount.to_be_bytes());
    out
}

pub fn decode_token_mint_args(input: &[u8]) -> Result<(Holder, u128), DexError> {
    let mut r = Reader::new(input);
    let to = r.holder()?;
    let amount = r.u128_be()?;
    r.finish()?;
    Ok((to, amount))
}
