//! SUBFROST AMM v0 — consensus-relevant constants.
//!
//! FROZEN for v0: any change to a value in this module is a new protocol
//! version and requires new golden vectors (`testdata/subfrost_amm_v0_vectors.json`).
//!
//! Economic model (SUBFROST standard):
//!   total swap fee  1.00%  (100 bps)
//!   LP share        0.80%  ( 80 bps)  — stays in reserves, accrues to LPs
//!   protocol share  0.20%  ( 20 bps)  — tracked in protocol fee accumulators

/// Basis-point denominator for all fee arithmetic.
pub const FEE_DENOMINATOR_BPS: u128 = 10_000;

/// Total swap fee taken from the input amount, in bps.
pub const TOTAL_SWAP_FEE_BPS: u128 = 100;

/// LP share of the swap fee, in bps.
pub const LP_FEE_BPS: u128 = 80;

/// Protocol share of the swap fee, in bps.
pub const PROTOCOL_FEE_BPS: u128 = 20;

/// LP units permanently locked at pool initialization.
/// They are minted to the pool itself and can never be redeemed.
pub const MINIMUM_LIQUIDITY: u128 = 1_000;

/// Domain-separation tag for the canonical pair key hash.
pub const PAIR_KEY_DOMAIN: &[u8] = b"zalkanes-subfrost-pair-v0";

const _: () = assert!(LP_FEE_BPS + PROTOCOL_FEE_BPS == TOTAL_SWAP_FEE_BPS);
const _: () = assert!(TOTAL_SWAP_FEE_BPS < FEE_DENOMINATOR_BPS);
const _: () = assert!(MINIMUM_LIQUIDITY > 0);
