//! Structured DEX events (spec §24).
//!
//! Events are emitted through the host and recorded only for calls that
//! ultimately succeed; a reverting call leaves no event trace. Encoding is
//! tag byte ++ fixed-width big-endian fields.

use alloc::vec::Vec;

use crate::error::DexError;
use crate::types::{AssetId, ContractId, Holder};

pub const TAG_POOL_CREATED: u8 = 0x01;
pub const TAG_LIQUIDITY_ADDED: u8 = 0x02;
pub const TAG_LIQUIDITY_REMOVED: u8 = 0x03;
pub const TAG_SWAP: u8 = 0x04;

/// Every observable DEX state transition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DexEvent {
    PoolCreated {
        factory: ContractId,
        pool: ContractId,
        token0: AssetId,
        token1: AssetId,
        amount0: u128,
        amount1: u128,
        lp_minted: u128,
    },
    LiquidityAdded {
        pool: ContractId,
        provider: Holder,
        amount0: u128,
        amount1: u128,
        lp_minted: u128,
    },
    LiquidityRemoved {
        pool: ContractId,
        provider: Holder,
        amount0: u128,
        amount1: u128,
        lp_burned: u128,
    },
    Swap {
        pool: ContractId,
        token_in: AssetId,
        token_out: AssetId,
        amount_in: u128,
        amount_out: u128,
        total_fee: u128,
        lp_fee: u128,
        protocol_fee: u128,
    },
}

impl DexEvent {
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(192);
        match self {
            DexEvent::PoolCreated {
                factory,
                pool,
                token0,
                token1,
                amount0,
                amount1,
                lp_minted,
            } => {
                out.push(TAG_POOL_CREATED);
                out.extend_from_slice(&factory.0);
                out.extend_from_slice(&pool.0);
                out.extend_from_slice(&token0.0);
                out.extend_from_slice(&token1.0);
                out.extend_from_slice(&amount0.to_be_bytes());
                out.extend_from_slice(&amount1.to_be_bytes());
                out.extend_from_slice(&lp_minted.to_be_bytes());
            }
            DexEvent::LiquidityAdded {
                pool,
                provider,
                amount0,
                amount1,
                lp_minted,
            } => {
                out.push(TAG_LIQUIDITY_ADDED);
                out.extend_from_slice(&pool.0);
                out.extend_from_slice(&provider.to_bytes());
                out.extend_from_slice(&amount0.to_be_bytes());
                out.extend_from_slice(&amount1.to_be_bytes());
                out.extend_from_slice(&lp_minted.to_be_bytes());
            }
            DexEvent::LiquidityRemoved {
                pool,
                provider,
                amount0,
                amount1,
                lp_burned,
            } => {
                out.push(TAG_LIQUIDITY_REMOVED);
                out.extend_from_slice(&pool.0);
                out.extend_from_slice(&provider.to_bytes());
                out.extend_from_slice(&amount0.to_be_bytes());
                out.extend_from_slice(&amount1.to_be_bytes());
                out.extend_from_slice(&lp_burned.to_be_bytes());
            }
            DexEvent::Swap {
                pool,
                token_in,
                token_out,
                amount_in,
                amount_out,
                total_fee,
                lp_fee,
                protocol_fee,
            } => {
                out.push(TAG_SWAP);
                out.extend_from_slice(&pool.0);
                out.extend_from_slice(&token_in.0);
                out.extend_from_slice(&token_out.0);
                out.extend_from_slice(&amount_in.to_be_bytes());
                out.extend_from_slice(&amount_out.to_be_bytes());
                out.extend_from_slice(&total_fee.to_be_bytes());
                out.extend_from_slice(&lp_fee.to_be_bytes());
                out.extend_from_slice(&protocol_fee.to_be_bytes());
            }
        }
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, DexError> {
        use crate::encode::Reader;
        if bytes.is_empty() {
            return Err(DexError::InvalidArguments);
        }
        let tag = bytes[0];
        let mut r = Reader::new(&bytes[1..]);
        let event = match tag {
            TAG_POOL_CREATED => DexEvent::PoolCreated {
                factory: r.contract()?,
                pool: r.contract()?,
                token0: r.asset()?,
                token1: r.asset()?,
                amount0: r.u128_be()?,
                amount1: r.u128_be()?,
                lp_minted: r.u128_be()?,
            },
            TAG_LIQUIDITY_ADDED => DexEvent::LiquidityAdded {
                pool: r.contract()?,
                provider: r.holder()?,
                amount0: r.u128_be()?,
                amount1: r.u128_be()?,
                lp_minted: r.u128_be()?,
            },
            TAG_LIQUIDITY_REMOVED => DexEvent::LiquidityRemoved {
                pool: r.contract()?,
                provider: r.holder()?,
                amount0: r.u128_be()?,
                amount1: r.u128_be()?,
                lp_burned: r.u128_be()?,
            },
            TAG_SWAP => DexEvent::Swap {
                pool: r.contract()?,
                token_in: r.asset()?,
                token_out: r.asset()?,
                amount_in: r.u128_be()?,
                amount_out: r.u128_be()?,
                total_fee: r.u128_be()?,
                lp_fee: r.u128_be()?,
                protocol_fee: r.u128_be()?,
            },
            _ => return Err(DexError::InvalidArguments),
        };
        r.finish()?;
        Ok(event)
    }
}
