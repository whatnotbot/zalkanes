//! Stable DEX error model with deterministic numeric codes.
//!
//! Codes are part of the v0 ABI: a contract call that fails returns the code
//! as its deterministic error payload. Never renumber; only append.

/// Every failure mode of the SUBFROST AMM v0.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u32)]
pub enum DexError {
    AlreadyInitialized = 1,
    NotInitialized = 2,
    UnauthorizedInitialize = 3,
    IdenticalAssets = 4,
    DuplicatePool = 5,
    InvalidAsset = 6,
    InvalidIncomingAssets = 7,
    ZeroAmount = 8,
    InsufficientInitialLiquidity = 9,
    InsufficientLiquidity = 10,
    InsufficientLp = 11,
    SlippageExceeded = 12,
    Expired = 13,
    ArithmeticOverflow = 14,
    ArithmeticUnderflow = 15,
    DivisionByZero = 16,
    InvalidOpcode = 17,
    InvalidArguments = 18,
    Reentrancy = 19,
    OutOfFuel = 20,
    Unsupported = 21,
}

impl DexError {
    /// Stable numeric wire code.
    #[must_use]
    pub const fn code(self) -> u32 {
        self as u32
    }

    /// Decode a wire code back into an error, if known.
    #[must_use]
    pub const fn from_code(code: u32) -> Option<Self> {
        Some(match code {
            1 => Self::AlreadyInitialized,
            2 => Self::NotInitialized,
            3 => Self::UnauthorizedInitialize,
            4 => Self::IdenticalAssets,
            5 => Self::DuplicatePool,
            6 => Self::InvalidAsset,
            7 => Self::InvalidIncomingAssets,
            8 => Self::ZeroAmount,
            9 => Self::InsufficientInitialLiquidity,
            10 => Self::InsufficientLiquidity,
            11 => Self::InsufficientLp,
            12 => Self::SlippageExceeded,
            13 => Self::Expired,
            14 => Self::ArithmeticOverflow,
            15 => Self::ArithmeticUnderflow,
            16 => Self::DivisionByZero,
            17 => Self::InvalidOpcode,
            18 => Self::InvalidArguments,
            19 => Self::Reentrancy,
            20 => Self::OutOfFuel,
            21 => Self::Unsupported,
            _ => return None,
        })
    }
}

impl core::fmt::Display for DexError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{self:?} (code {})", self.code())
    }
}
