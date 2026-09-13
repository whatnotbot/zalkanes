//! MATH-001..054 — pure AMM arithmetic unit-test matrix (spec §27).

use zalkanes_dex_core::error::DexError;
use zalkanes_dex_core::math::{
    initial_liquidity, quote_add_liquidity, quote_remove_liquidity, quote_swap_exact_in, split_fee,
};
use zalkanes_dex_core::v0::MINIMUM_LIQUIDITY;
use zalkanes_dex_core::wide::{integer_sqrt, mul_div_floor, mul_wide, sqrt_wide};

// ── integer sqrt ─────────────────────────────────────────────────────────

#[test]
fn math_001_sqrt_zero() {
    assert_eq!(integer_sqrt(0), 0);
}

#[test]
fn math_002_sqrt_one() {
    assert_eq!(integer_sqrt(1), 1);
}

#[test]
fn math_003_sqrt_two() {
    assert_eq!(integer_sqrt(2), 1);
}

#[test]
fn math_004_sqrt_three() {
    assert_eq!(integer_sqrt(3), 1);
}

#[test]
fn math_005_sqrt_four() {
    assert_eq!(integer_sqrt(4), 2);
}

#[test]
fn math_006_perfect_squares_across_range() {
    let roots: [u128; 12] = [
        5,
        10,
        1_000,
        65_535,
        65_536,
        1 << 32,
        (1 << 32) + 1,
        1_000_000_007,
        1 << 63,
        (1 << 64) - 2,
        (1 << 64) - 1,
        123_456_789_012_345,
    ];
    for r in roots {
        assert_eq!(integer_sqrt(r * r), r, "sqrt of {r}^2");
        if r > 1 {
            assert_eq!(integer_sqrt(r * r - 1), r - 1, "sqrt of {r}^2 - 1");
            assert_eq!(integer_sqrt(r * r + 1), r, "sqrt of {r}^2 + 1");
        }
    }
}

#[test]
fn math_007_sqrt_floor_property() {
    let samples: [u128; 10] = [
        0,
        1,
        2,
        7,
        999_999_999_999,
        u128::from(u64::MAX),
        u128::from(u64::MAX) + 1,
        u128::MAX / 2,
        u128::MAX - 1,
        u128::MAX,
    ];
    for x in samples {
        let s = integer_sqrt(x);
        // s*s <= x, overflow-safe: s <= 2^64-1 so s*s fits u128.
        assert!(
            s.checked_mul(s).expect("s*s fits") <= x,
            "floor lower bound for {x}"
        );
        // (s+1)^2 > x, checked via wide product to survive s = 2^64-1.
        let (hi, lo) = mul_wide(s + 1, s + 1);
        assert!((hi, lo) > (0, x), "floor upper bound for {x}");
    }
}

#[test]
fn math_008_sqrt_wide_boundaries() {
    // Largest possible 256-bit product: (u128::MAX)^2 -> root u128::MAX.
    let (hi, lo) = mul_wide(u128::MAX, u128::MAX);
    assert_eq!(sqrt_wide(hi, lo), u128::MAX);
    // One below a wide perfect square.
    let r = 1u128 << 100;
    let (hi, lo) = mul_wide(r, r);
    assert_eq!(sqrt_wide(hi, lo), r);
    let (hi2, lo2) = if lo == 0 {
        (hi - 1, u128::MAX)
    } else {
        (hi, lo - 1)
    };
    assert_eq!(sqrt_wide(hi2, lo2), r - 1);
    // Boundary between narrow and wide paths.
    assert_eq!(sqrt_wide(0, u128::MAX), integer_sqrt(u128::MAX));
    assert_eq!(sqrt_wide(1, 0), (1u128 << 64));
}

// ── initial liquidity ────────────────────────────────────────────────────

#[test]
fn math_010_minimum_exactly_insufficient() {
    // sqrt(1000*1000) = 1000 == MINIMUM_LIQUIDITY -> rejected.
    assert_eq!(
        initial_liquidity(1_000, 1_000),
        Err(DexError::InsufficientInitialLiquidity)
    );
}

#[test]
fn math_011_just_above_minimum() {
    // sqrt(1002*1001) = sqrt(1003002) = 1001 -> provider gets 1.
    let (provider, gross) = initial_liquidity(1_002, 1_001).expect("valid");
    assert_eq!(gross, 1_001);
    assert_eq!(provider, 1);
}

#[test]
fn math_012_equal_reserves() {
    let (provider, gross) = initial_liquidity(1_000_000, 1_000_000).expect("valid");
    assert_eq!(gross, 1_000_000);
    assert_eq!(provider, 1_000_000 - MINIMUM_LIQUIDITY);
}

#[test]
fn math_013_asymmetric_reserves() {
    // sqrt(1 * 10^13) = floor(3162277.66..) = 3162277
    let (provider, gross) = initial_liquidity(1, 10_000_000_000_000).expect("valid");
    assert_eq!(gross, 3_162_277);
    assert_eq!(provider, 3_162_277 - MINIMUM_LIQUIDITY);
}

#[test]
fn math_014_zero_amount_fails() {
    assert_eq!(initial_liquidity(0, 10_000), Err(DexError::ZeroAmount));
    assert_eq!(initial_liquidity(10_000, 0), Err(DexError::ZeroAmount));
}

#[test]
fn math_015_wide_product_handled() {
    // 2^100 * 2^100 = 2^200 overflows u128 but sqrt = 2^100 fits.
    let (provider, gross) = initial_liquidity(1 << 100, 1 << 100).expect("wide path");
    assert_eq!(gross, 1u128 << 100);
    assert_eq!(provider, (1u128 << 100) - MINIMUM_LIQUIDITY);
}

// ── add liquidity ────────────────────────────────────────────────────────

#[test]
fn math_020_exact_ratio() {
    let o = quote_add_liquidity(1_000_000, 2_000_000, 1_414_213, 100_000, 200_000).expect("ok");
    assert_eq!((o.accepted0, o.accepted1), (100_000, 200_000));
    assert_eq!((o.refund0, o.refund1), (0, 0));
    assert_eq!(o.lp_minted, 141_421); // floor(100_000 * 1_414_213 / 1_000_000)
}

#[test]
fn math_021_token0_excess_refunded() {
    let o = quote_add_liquidity(1_000_000, 1_000_000, 1_000_000, 700_000, 500_000).expect("ok");
    assert_eq!((o.accepted0, o.accepted1), (500_000, 500_000));
    assert_eq!((o.refund0, o.refund1), (200_000, 0));
    assert_eq!(o.lp_minted, 500_000);
}

#[test]
fn math_022_token1_excess_refunded() {
    let o = quote_add_liquidity(1_000_000, 1_000_000, 1_000_000, 500_000, 700_000).expect("ok");
    assert_eq!((o.accepted0, o.accepted1), (500_000, 500_000));
    assert_eq!((o.refund0, o.refund1), (0, 200_000));
    assert_eq!(o.lp_minted, 500_000);
}

#[test]
fn math_023_lp_rounding_floors() {
    // reserves 7:13, supply 1009, desired (3,5):
    // optimal1 = floor(3*13/7) = 5 <= 5 -> accept (3,5) (upstream branch rule)
    // lp = min(floor(3*1009/7)=432, floor(5*1009/13)=388) = 388 (floor visible)
    let o = quote_add_liquidity(7, 13, 1_009, 3, 5).expect("ok");
    assert_eq!((o.accepted0, o.accepted1), (3, 5));
    assert_eq!((o.refund0, o.refund1), (0, 0));
    assert_eq!(o.lp_minted, 388);
}

#[test]
fn math_024_minimum_nonzero_lp() {
    let o = quote_add_liquidity(1_000_000, 1_000_000, 1_000_000, 1, 1).expect("ok");
    assert_eq!(o.lp_minted, 1);
    assert_eq!((o.accepted0, o.accepted1), (1, 1));
}

#[test]
fn math_025_large_reserves() {
    let r = 1u128 << 90;
    let o = quote_add_liquidity(r, r, r, 1 << 80, (1 << 80) + 12_345).expect("ok");
    assert_eq!(o.accepted0, 1 << 80);
    assert_eq!(o.accepted1, 1 << 80);
    assert_eq!(o.refund1, 12_345);
    assert_eq!(o.lp_minted, 1 << 80);
}

#[test]
fn math_026_zero_reserve_rejected() {
    assert_eq!(
        quote_add_liquidity(0, 1_000, 1_000, 10, 10),
        Err(DexError::NotInitialized)
    );
    assert_eq!(
        quote_add_liquidity(1_000, 0, 1_000, 10, 10),
        Err(DexError::NotInitialized)
    );
}

// ── remove liquidity ─────────────────────────────────────────────────────

#[test]
fn math_030_proportional_half_burn() {
    let (a0, a1) = quote_remove_liquidity(1_000_000, 2_000_000, 1_000_000, 500_000).expect("ok");
    assert_eq!((a0, a1), (500_000, 1_000_000));
}

#[test]
fn math_031_one_unit_burn() {
    let (a0, a1) = quote_remove_liquidity(1_000_000, 2_000_000, 1_000_000, 1).expect("ok");
    assert_eq!((a0, a1), (1, 2));
}

#[test]
fn math_032_rounding_floors() {
    // 7 LP of 1_000_000 supply over reserves (999_999, 999_999):
    // floor(7*999_999/1_000_000) = 6 on both sides — floor visible.
    let (a0, a1) = quote_remove_liquidity(999_999, 999_999, 1_000_000, 7).expect("ok");
    assert_eq!((a0, a1), (6, 6));
    // A burn whose floored output is zero on either side is rejected
    // (upstream INSUFFICIENT_LIQUIDITY_BURNED parity).
    assert_eq!(
        quote_remove_liquidity(7, 13, 1_009, 9),
        Err(DexError::InsufficientLiquidity)
    );
}

#[test]
fn math_033_locked_minimum_unredeemable() {
    // All circulating LP (supply - MINIMUM_LIQUIDITY) cannot withdraw everything.
    let supply = 1_000_000u128;
    let circulating = supply - MINIMUM_LIQUIDITY;
    let (a0, a1) = quote_remove_liquidity(1_000_000, 1_000_000, supply, circulating).expect("ok");
    assert!(a0 < 1_000_000 && a1 < 1_000_000);
    assert_eq!((a0, a1), (999_000, 999_000));
}

#[test]
fn math_034_excessive_lp_rejected() {
    assert_eq!(
        quote_remove_liquidity(1_000, 1_000, 1_000, 1_001),
        Err(DexError::InsufficientLp)
    );
}

// ── swaps ────────────────────────────────────────────────────────────────

#[test]
fn math_040_swap_a_to_b() {
    // in=10_000: total=100, lp=80, proto=20, eff=9_900
    // out = floor(9900*1_000_000 / 1_009_900) = 9802
    let o = quote_swap_exact_in(1_000_000, 1_000_000, 10_000).expect("ok");
    assert_eq!(o.amount_out, 9_802);
    assert_eq!(o.fees.total_fee, 100);
    assert_eq!(o.fees.lp_fee, 80);
    assert_eq!(o.fees.protocol_fee, 20);
}

#[test]
fn math_041_swap_b_to_a() {
    // Asymmetric reserves, other direction.
    let o = quote_swap_exact_in(2_750_161, 999_983, 137_251).expect("ok");
    // wf = 137_251*9900; out = floor(wf*999_983 / (2_750_161*10_000 + wf)) = 47_080
    assert_eq!(o.fees.total_fee, 1_372);
    assert_eq!(o.amount_out, 47_080);
}

#[test]
fn math_042_dust_rejected() {
    // fee rounds to zero -> reject
    assert_eq!(
        quote_swap_exact_in(1_000_000, 1_000_000, 99),
        Err(DexError::ZeroAmount)
    );
    // output rounds to zero -> reject
    assert_eq!(
        quote_swap_exact_in(1_000_000_000, 3, 1_000_000),
        Err(DexError::InsufficientLiquidity)
    );
}

#[test]
fn math_043_medium_trade() {
    let o = quote_swap_exact_in(1_000_000, 1_000_000, 12_345).expect("ok");
    // total=123; wf=12_345*9900; out = floor(wf*1e6/(1e10+wf)) = 12_073
    assert_eq!(o.fees.total_fee, 123);
    assert_eq!(o.amount_out, 12_073);
}

#[test]
fn math_044_large_price_impact() {
    // Swap equal to the whole reserve:
    // out = floor(9_900_000_000*1e6/(1e10+9_900_000_000)) = 497_487
    let o = quote_swap_exact_in(1_000_000, 1_000_000, 1_000_000).expect("ok");
    assert_eq!(o.amount_out, 497_487);
    assert!(o.amount_out < 1_000_000);
}

#[test]
fn math_045_output_never_drains_reserve() {
    let cases = [
        (1u128, 1_000_000_000_000u128, u128::MAX / 4),
        (1, 2, 1_000_000),
        (1_000, u128::MAX / 2, u128::MAX / 4),
    ];
    for (ri, ro, ain) in cases {
        if let Ok(o) = quote_swap_exact_in(ri, ro, ain) {
            assert!(o.amount_out < ro, "reserve drained for {ri},{ro},{ain}");
        }
    }
}

#[test]
fn math_046_zero_input_fails() {
    assert_eq!(
        quote_swap_exact_in(1_000_000, 1_000_000, 0),
        Err(DexError::ZeroAmount)
    );
}

#[test]
fn math_047_zero_reserve_fails() {
    assert_eq!(
        quote_swap_exact_in(0, 1_000_000, 10_000),
        Err(DexError::InsufficientLiquidity)
    );
    assert_eq!(
        quote_swap_exact_in(1_000_000, 0, 10_000),
        Err(DexError::InsufficientLiquidity)
    );
}

#[test]
fn math_048_protocol_fee_exact_split() {
    let f = split_fee(1_000_000).expect("ok");
    assert_eq!(f.total_fee, 10_000);
    assert_eq!(f.lp_fee, 8_000);
    assert_eq!(f.protocol_fee, 2_000);
    assert_eq!(f.lp_fee + f.protocol_fee, f.total_fee);
}

#[test]
fn math_049_fee_rounding_below_one_unit() {
    let f = split_fee(99).expect("ok");
    assert_eq!((f.total_fee, f.lp_fee, f.protocol_fee), (0, 0, 0));
}

#[test]
fn math_050_fee_rounding_exact_boundary() {
    // in=100: total=1, lp=floor(100*80/10000)=0, protocol=1
    let f = split_fee(100).expect("ok");
    assert_eq!((f.total_fee, f.lp_fee, f.protocol_fee), (1, 0, 1));
}

#[test]
fn math_051_fee_rounding_above_boundary() {
    // in=125: total=1, lp=1, protocol=0
    let f = split_fee(125).expect("ok");
    assert_eq!((f.total_fee, f.lp_fee, f.protocol_fee), (1, 1, 0));
    // in=250: total=2, lp=2, protocol=0
    let f = split_fee(250).expect("ok");
    assert_eq!((f.total_fee, f.lp_fee, f.protocol_fee), (2, 2, 0));
}

#[test]
fn math_052_large_u128_safe_case() {
    let o = quote_swap_exact_in(1u128 << 100, 1u128 << 100, 1u128 << 90).expect("ok");
    assert!(o.amount_out > 0 && o.amount_out < 1u128 << 100);
}

#[test]
fn math_053_wide_intermediate_required() {
    // effective_in * reserve_out overflows u128; must still succeed.
    let ri = 10u128.pow(30);
    let ro = 10u128.pow(30);
    let ain = 10u128.pow(27);
    let o = quote_swap_exact_in(ri, ro, ain).expect("wide path");
    assert!(o.amount_out > 0 && o.amount_out < ro);
}

#[test]
fn math_054_overflow_fails_deterministically() {
    // reserve_in + effective_in overflows u128.
    assert_eq!(
        quote_swap_exact_in(u128::MAX - 1, 1_000_000, 1_000_000),
        Err(DexError::ArithmeticOverflow)
    );
    // mul_div quotient overflow is also an error, not a panic.
    assert_eq!(
        mul_div_floor(u128::MAX, u128::MAX, 1),
        Err(DexError::ArithmeticOverflow)
    );
    assert_eq!(mul_div_floor(1, 1, 0), Err(DexError::DivisionByZero));
}
