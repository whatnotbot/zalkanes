//! Deterministic wide (256-bit intermediate) integer helpers.
//!
//! Consensus-critical: no floating point, no `as` truncation, no panics on
//! adversarial input. Every fallible operation returns `Result`.

use crate::error::DexError;

/// Full 256-bit product of two u128 values as (hi, lo) limbs.
#[must_use]
pub fn mul_wide(a: u128, b: u128) -> (u128, u128) {
    const MASK: u128 = (1u128 << 64) - 1;
    let (a_hi, a_lo) = (a >> 64, a & MASK);
    let (b_hi, b_lo) = (b >> 64, b & MASK);

    let ll = a_lo * b_lo;
    let lh = a_lo * b_hi;
    let hl = a_hi * b_lo;
    let hh = a_hi * b_hi;

    let mid = (ll >> 64) + (lh & MASK) + (hl & MASK);
    let lo = (mid << 64) | (ll & MASK);
    let hi = hh + (lh >> 64) + (hl >> 64) + (mid >> 64);
    (hi, lo)
}

/// floor((a * b) / d) with a 256-bit intermediate product.
///
/// Errors: `DivisionByZero` if `d == 0`; `ArithmeticOverflow` if the quotient
/// does not fit in u128.
pub fn mul_div_floor(a: u128, b: u128, d: u128) -> Result<u128, DexError> {
    if d == 0 {
        return Err(DexError::DivisionByZero);
    }
    let (hi, lo) = mul_wide(a, b);
    if hi == 0 {
        return Ok(lo / d);
    }
    if hi >= d {
        return Err(DexError::ArithmeticOverflow);
    }
    // Restoring binary long division of the 256-bit value (hi:lo) by d.
    // Invariant: rem < d at the top of every iteration, so the true shifted
    // value 2*rem + bit < 2*d needs at most one subtraction.
    let mut rem = hi;
    let mut q: u128 = 0;
    let mut i = 128u32;
    while i > 0 {
        i -= 1;
        let carry = rem >> 127;
        rem = (rem << 1) | ((lo >> i) & 1);
        q <<= 1;
        if carry == 1 || rem >= d {
            rem = rem.wrapping_sub(d);
            q |= 1;
        }
    }
    Ok(q)
}

/// floor(sqrt(hi:lo)) of a 256-bit value; the root always fits in u128.
#[must_use]
pub fn sqrt_wide(hi: u128, lo: u128) -> u128 {
    if hi == 0 {
        return integer_sqrt(lo);
    }
    // Binary search the largest s with s*s <= (hi:lo).
    let mut low: u128 = 1;
    let mut high: u128 = u128::MAX;
    while low < high {
        // mid = ceil((low + high) / 2) without overflow
        let mid = low + (high - low) / 2 + ((high - low) % 2);
        let (sq_hi, sq_lo) = mul_wide(mid, mid);
        if (sq_hi, sq_lo) <= (hi, lo) {
            low = mid;
        } else {
            high = mid - 1;
        }
    }
    low
}

/// floor(sqrt(x)) for u128. Deterministic binary search; no floating point.
#[must_use]
pub fn integer_sqrt(x: u128) -> u128 {
    if x < 2 {
        return x;
    }
    const MAX_ROOT: u128 = (1u128 << 64) - 1;
    if x >= MAX_ROOT * MAX_ROOT {
        return MAX_ROOT;
    }
    let mut low: u128 = 1;
    let mut high: u128 = MAX_ROOT - 1;
    while low < high {
        let mid = low + (high - low) / 2 + ((high - low) % 2);
        if mid * mid <= x {
            low = mid;
        } else {
            high = mid - 1;
        }
    }
    low
}

/// Compare a*b vs c*d using 256-bit products (no overflow).
#[must_use]
pub fn cmp_mul(a: u128, b: u128, c: u128, d: u128) -> core::cmp::Ordering {
    mul_wide(a, b).cmp(&mul_wide(c, d))
}
