//! Canonical DEX types.
//!
//! `ContractId` is reused verbatim from `zalkanes-core` — no parallel
//! contract-identity type exists in the DEX. `AssetId` is new: the frozen
//! Zalkanes v0 protocol has no asset primitive, so the DEX defines the
//! canonical 32-byte asset identity here as part of the proposed ABI
//! extension. A contract-native asset's id is the contract's id bytes,
//! which makes the LP token identity exactly the pool's `ContractId`.

use alloc::vec::Vec;

use crate::error::DexError;
use crate::v0::PAIR_KEY_DOMAIN;
pub use zalkanes_core::types::ContractId;

/// Canonical 32-byte asset identity (proposed ABI extension).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AssetId(pub [u8; 32]);

impl AssetId {
    /// The native asset of a contract: identical bytes to its `ContractId`.
    #[must_use]
    pub const fn of_contract(id: &ContractId) -> Self {
        Self(id.0)
    }

    /// Lowercase hex, no prefix (platform convention).
    #[must_use]
    pub fn as_hex(&self) -> alloc::string::String {
        let mut s = alloc::string::String::with_capacity(64);
        for b in self.0 {
            use core::fmt::Write;
            let _ = write!(s, "{b:02x}");
        }
        s
    }
}

impl core::fmt::Display for AssetId {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(&self.as_hex())
    }
}

/// 32-byte external account identity (proposed ABI extension). On a real
/// chain this would be derived from the funding signer; in the DEX testkit
/// it is an opaque deterministic fixture value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AccountId(pub [u8; 32]);

/// Anything that can hold asset balances.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Holder {
    External(AccountId),
    Contract(ContractId),
}

// `zalkanes_core::ContractId` does not derive `Ord`; order holders by their
// canonical 33-byte wire form so ledger iteration stays deterministic.
impl Ord for Holder {
    fn cmp(&self, other: &Self) -> core::cmp::Ordering {
        self.to_bytes().cmp(&other.to_bytes())
    }
}

impl PartialOrd for Holder {
    fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Holder {
    /// 33-byte wire form: tag (0 external / 1 contract) ++ 32 id bytes.
    #[must_use]
    pub fn to_bytes(&self) -> [u8; 33] {
        let mut out = [0u8; 33];
        match self {
            Holder::External(a) => {
                out[0] = 0;
                out[1..].copy_from_slice(&a.0);
            }
            Holder::Contract(c) => {
                out[0] = 1;
                out[1..].copy_from_slice(&c.0);
            }
        }
        out
    }

    /// Decode the 33-byte wire form.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, DexError> {
        if bytes.len() != 33 {
            return Err(DexError::InvalidArguments);
        }
        let mut id = [0u8; 32];
        id.copy_from_slice(&bytes[1..33]);
        match bytes[0] {
            0 => Ok(Holder::External(AccountId(id))),
            1 => Ok(Holder::Contract(ContractId(id))),
            _ => Err(DexError::InvalidArguments),
        }
    }
}

/// A canonically ordered, distinct asset pair.
///
/// Invariant: `token0 < token1` in canonical byte order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Pair {
    pub token0: AssetId,
    pub token1: AssetId,
}

impl Pair {
    /// Canonicalize an unordered pair. Identical assets are rejected.
    ///
    /// `canonical(a, b) == canonical(b, a)` for all distinct `a`, `b`.
    pub fn canonical(a: AssetId, b: AssetId) -> Result<Self, DexError> {
        match a.cmp(&b) {
            core::cmp::Ordering::Equal => Err(DexError::IdenticalAssets),
            core::cmp::Ordering::Less => Ok(Pair {
                token0: a,
                token1: b,
            }),
            core::cmp::Ordering::Greater => Ok(Pair {
                token0: b,
                token1: a,
            }),
        }
    }

    /// Domain-separated canonical pair key:
    /// `"zalkanes-subfrost-pair-v0" ‖ token0 ‖ token1`.
    ///
    /// Fixed-width fields make this collision-free without hashing.
    #[must_use]
    pub fn key(&self) -> Vec<u8> {
        let mut key = Vec::with_capacity(PAIR_KEY_DOMAIN.len() + 64);
        key.extend_from_slice(PAIR_KEY_DOMAIN);
        key.extend_from_slice(&self.token0.0);
        key.extend_from_slice(&self.token1.0);
        key
    }
}

/// Mutable pool accounting state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PoolState {
    pub token0: AssetId,
    pub token1: AssetId,
    pub reserve0: u128,
    pub reserve1: u128,
    pub total_lp_supply: u128,
    pub protocol_fees0: u128,
    pub protocol_fees1: u128,
}

/// Full pool view (opcode 999).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PoolDetails {
    pub pool_id: ContractId,
    pub factory_id: ContractId,
    pub token0: AssetId,
    pub token1: AssetId,
    pub reserve0: u128,
    pub reserve1: u128,
    pub total_lp_supply: u128,
    pub protocol_fees0: u128,
    pub protocol_fees1: u128,
}

/// Exact-input swap quote.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SwapQuote {
    pub token_in: AssetId,
    pub token_out: AssetId,
    pub amount_in: u128,
    pub amount_out: u128,
    pub total_fee: u128,
    pub lp_fee: u128,
    pub protocol_fee: u128,
}

/// Liquidity operation quote.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LiquidityQuote {
    pub amount0: u128,
    pub amount1: u128,
    pub lp_amount: u128,
}
