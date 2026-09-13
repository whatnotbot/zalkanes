//! PROP-001..020 — deterministic property tests (spec §29).
//!
//! Uses a fixed-seed xorshift generator instead of an external property
//! crate so the workspace gains no new dependencies and every CI run
//! exercises the identical case set.

use zalkanes_dex_core::error::DexError;
use zalkanes_dex_core::math::{
    initial_liquidity, quote_add_liquidity, quote_remove_liquidity, quote_swap_exact_in,
};
use zalkanes_dex_core::types::{AssetId, Pair};
use zalkanes_dex_core::v0::MINIMUM_LIQUIDITY;

const CASES: usize = 2_000;

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        // xorshift64* — deterministic, no external crate.
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn u128_up_to_bits(&mut self, bits: u32) -> u128 {
        let raw = (u128::from(self.next()) << 64) | u128::from(self.next());
        if bits >= 128 {
            raw
        } else {
            raw & ((1u128 << bits) - 1)
        }
    }

    fn asset(&mut self) -> AssetId {
        let mut bytes = [0u8; 32];
        for chunk in bytes.chunks_mut(8) {
            chunk.copy_from_slice(&self.next().to_be_bytes());
        }
        AssetId(bytes)
    }
}

/// A pool accounting model driven purely by the frozen v0 math. Mirrors
/// the contract's state transitions so conservation properties can be
/// checked over long random operation sequences.
#[derive(Clone, Debug, PartialEq, Eq)]
struct PoolModel {
    reserve0: u128,
    reserve1: u128,
    supply: u128,
    pfees0: u128,
    pfees1: u128,
    // Assets held by "everyone else" (for conservation checks).
    outside0: u128,
    outside1: u128,
    outside_lp: u128,
}

impl PoolModel {
    fn init(a0: u128, a1: u128, budget0: u128, budget1: u128) -> Option<Self> {
        let (provider_lp, gross) = initial_liquidity(a0, a1).ok()?;
        Some(PoolModel {
            reserve0: a0,
            reserve1: a1,
            supply: gross,
            pfees0: 0,
            pfees1: 0,
            outside0: budget0 - a0,
            outside1: budget1 - a1,
            outside_lp: provider_lp,
        })
    }

    fn total0(&self) -> u128 {
        self.reserve0 + self.pfees0 + self.outside0
    }

    fn total1(&self) -> u128 {
        self.reserve1 + self.pfees1 + self.outside1
    }

    /// Circulating LP plus the permanently locked minimum equals supply.
    fn lp_conserved(&self) -> bool {
        self.outside_lp + MINIMUM_LIQUIDITY == self.supply
    }
}

#[test]
fn prop_001_canonical_pair_commutes() {
    let mut rng = Rng(0x5eed_0001);
    for _ in 0..CASES {
        let a = rng.asset();
        let b = rng.asset();
        if a == b {
            continue;
        }
        let ab = Pair::canonical(a, b).expect("distinct");
        let ba = Pair::canonical(b, a).expect("distinct");
        assert_eq!(ab, ba);
        assert_eq!(ab.key(), ba.key());
    }
}

#[test]
fn prop_002_pair_tokens_distinct() {
    let mut rng = Rng(0x5eed_0002);
    for _ in 0..CASES {
        let a = rng.asset();
        assert_eq!(Pair::canonical(a, a), Err(DexError::IdenticalAssets));
        let b = rng.asset();
        if let Ok(pair) = Pair::canonical(a, b) {
            assert_ne!(pair.token0, pair.token1);
            assert!(pair.token0 < pair.token1);
        }
    }
}

#[test]
fn prop_003_duplicate_canonical_pair_single_key() {
    // Registration keyed by Pair::key(): both orderings of the same pair
    // always produce the identical key, so a keyed registry can never hold
    // two pools for one pair. (Runtime enforcement is FACTORY-004.)
    let mut rng = Rng(0x5eed_0003);
    let mut keys = std::collections::BTreeSet::new();
    let mut pairs = 0u32;
    for _ in 0..CASES {
        let a = rng.asset();
        let b = rng.asset();
        if let Ok(pair) = Pair::canonical(a, b) {
            pairs += 1;
            keys.insert(pair.key());
            keys.insert(Pair::canonical(b, a).expect("distinct").key());
        }
    }
    assert_eq!(keys.len() as u32, pairs, "one key per unordered pair");
}

#[test]
fn prop_004_005_swap_output_bounds() {
    let mut rng = Rng(0x5eed_0004);
    for _ in 0..CASES {
        let ri = rng.u128_up_to_bits(100).max(1);
        let ro = rng.u128_up_to_bits(100).max(1);
        let ain = rng.u128_up_to_bits(96);
        if let Ok(o) = quote_swap_exact_in(ri, ro, ain) {
            assert!(o.amount_out > 0, "PROP-004");
            assert!(o.amount_out < ro, "PROP-005: {ri} {ro} {ain}");
        }
    }
}

#[test]
fn prop_006_to_014_state_machine_conservation() {
    let mut rng = Rng(0x5eed_0006);
    for _case in 0..200 {
        let budget0 = rng.u128_up_to_bits(80).max(2_000_000);
        let budget1 = rng.u128_up_to_bits(80).max(2_000_000);
        let a0 = (rng.u128_up_to_bits(60) % (budget0 / 2)).max(10_000);
        let a1 = (rng.u128_up_to_bits(60) % (budget1 / 2)).max(10_000);
        let Some(mut pool) = PoolModel::init(a0, a1, budget0, budget1) else {
            continue;
        };
        let (t0_start, t1_start) = (pool.total0(), pool.total1());
        assert!(pool.lp_conserved(), "PROP-014 at init");

        for _step in 0..40 {
            let before = pool.clone();
            match rng.next() % 3 {
                // add liquidity (PROP-009: reserves never decrease)
                0 => {
                    let d0 = rng.u128_up_to_bits(50) % (pool.outside0 + 1);
                    let d1 = rng.u128_up_to_bits(50) % (pool.outside1 + 1);
                    match quote_add_liquidity(pool.reserve0, pool.reserve1, pool.supply, d0, d1) {
                        Ok(o) => {
                            pool.reserve0 += o.accepted0;
                            pool.reserve1 += o.accepted1;
                            pool.supply += o.lp_minted;
                            pool.outside0 -= o.accepted0;
                            pool.outside1 -= o.accepted1;
                            pool.outside_lp += o.lp_minted;
                            assert!(pool.reserve0 >= before.reserve0, "PROP-009");
                            assert!(pool.reserve1 >= before.reserve1, "PROP-009");
                            assert!(pool.supply > before.supply, "PROP-007");
                            // PROP-011: immediate burn of what was minted
                            // cannot return more than was deposited.
                            let (b0, b1) = quote_remove_liquidity(
                                pool.reserve0,
                                pool.reserve1,
                                pool.supply,
                                o.lp_minted,
                            )
                            .unwrap_or((0, 0));
                            assert!(b0 <= o.accepted0, "PROP-011 token0");
                            assert!(b1 <= o.accepted1, "PROP-011 token1");
                        }
                        Err(_) => assert_eq!(pool, before, "PROP-020"),
                    }
                }
                // remove liquidity (PROP-010: reserves never increase)
                1 => {
                    let lp = rng.u128_up_to_bits(50) % (pool.outside_lp + 1);
                    match quote_remove_liquidity(pool.reserve0, pool.reserve1, pool.supply, lp) {
                        Ok((o0, o1)) => {
                            assert!(lp <= pool.outside_lp, "cannot burn locked minimum");
                            pool.reserve0 -= o0;
                            pool.reserve1 -= o1;
                            pool.supply -= lp;
                            pool.outside0 += o0;
                            pool.outside1 += o1;
                            pool.outside_lp -= lp;
                            assert!(pool.reserve0 <= before.reserve0, "PROP-010");
                            assert!(pool.reserve1 <= before.reserve1, "PROP-010");
                            assert!(pool.supply >= MINIMUM_LIQUIDITY, "PROP-007");
                        }
                        Err(_) => assert_eq!(pool, before, "PROP-020"),
                    }
                }
                // swap token0 -> token1 (PROP-006/008/012/013)
                _ => {
                    let ain = rng.u128_up_to_bits(48) % (pool.outside0 + 1);
                    match quote_swap_exact_in(pool.reserve0, pool.reserve1, ain) {
                        Ok(o) => {
                            assert!(o.amount_out <= pool.reserve1, "PROP-006");
                            pool.reserve0 += ain - o.fees.protocol_fee;
                            pool.pfees0 += o.fees.protocol_fee;
                            pool.reserve1 -= o.amount_out;
                            pool.outside0 -= ain;
                            pool.outside1 += o.amount_out;
                            assert!(pool.pfees0 >= before.pfees0, "PROP-008");
                        }
                        Err(_) => assert_eq!(pool, before, "PROP-020"),
                    }
                }
            }
            // PROP-012/013: no operation creates either asset.
            assert_eq!(pool.total0(), t0_start, "PROP-013 token0 conservation");
            assert_eq!(pool.total1(), t1_start, "PROP-013 token1 conservation");
            assert!(pool.lp_conserved(), "PROP-014 LP conservation");
        }
    }
}

#[test]
fn prop_015_determinism_same_inputs_same_outputs() {
    let mut rng = Rng(0x5eed_0015);
    for _ in 0..CASES {
        let ri = rng.u128_up_to_bits(90).max(1);
        let ro = rng.u128_up_to_bits(90).max(1);
        let ain = rng.u128_up_to_bits(80);
        assert_eq!(
            quote_swap_exact_in(ri, ro, ain),
            quote_swap_exact_in(ri, ro, ain),
            "PROP-015"
        );
        let s = rng.u128_up_to_bits(90).max(1);
        let d0 = rng.u128_up_to_bits(60);
        let d1 = rng.u128_up_to_bits(60);
        assert_eq!(
            quote_add_liquidity(ri, ro, s, d0, d1),
            quote_add_liquidity(ri, ro, s, d0, d1),
            "PROP-015"
        );
    }
}

#[test]
fn prop_016_output_monotone_in_amount_in() {
    let mut rng = Rng(0x5eed_0016);
    for _ in 0..500 {
        let ri = rng.u128_up_to_bits(70).max(1_000_000);
        let ro = rng.u128_up_to_bits(70).max(1_000_000);
        let base = (rng.u128_up_to_bits(40)).max(100);
        let step = (rng.u128_up_to_bits(16)).max(1);
        let out_a = quote_swap_exact_in(ri, ro, base).map(|o| o.amount_out);
        let out_b = quote_swap_exact_in(ri, ro, base + step).map(|o| o.amount_out);
        if let (Ok(a), Ok(b)) = (out_a, out_b) {
            assert!(b >= a, "PROP-016: {ri} {ro} {base} +{step}");
        }
    }
}

#[test]
fn prop_017_output_monotone_in_reserve_out() {
    let mut rng = Rng(0x5eed_0017);
    for _ in 0..500 {
        let ri = rng.u128_up_to_bits(70).max(1_000_000);
        let ro = rng.u128_up_to_bits(70).max(1_000_000);
        let step = (rng.u128_up_to_bits(30)).max(1);
        let ain = (rng.u128_up_to_bits(40)).max(100);
        let out_a = quote_swap_exact_in(ri, ro, ain).map(|o| o.amount_out);
        let out_b = quote_swap_exact_in(ri, ro + step, ain).map(|o| o.amount_out);
        if let (Ok(a), Ok(b)) = (out_a, out_b) {
            assert!(b >= a, "PROP-017");
        }
    }
}

#[test]
fn prop_018_output_antitone_in_reserve_in() {
    let mut rng = Rng(0x5eed_0018);
    for _ in 0..500 {
        let ri = rng.u128_up_to_bits(70).max(1_000_000);
        let ro = rng.u128_up_to_bits(70).max(1_000_000);
        let step = (rng.u128_up_to_bits(30)).max(1);
        let ain = (rng.u128_up_to_bits(40)).max(100);
        let out_a = quote_swap_exact_in(ri, ro, ain).map(|o| o.amount_out);
        let out_b = quote_swap_exact_in(ri + step, ro, ain).map(|o| o.amount_out);
        if let (Ok(a), Ok(b)) = (out_a, out_b) {
            assert!(b <= a, "PROP-018");
        }
    }
}

#[test]
fn prop_019_success_never_overflows_visibly() {
    // Adversarial extreme inputs: success implies internally consistent
    // in-range outputs; failure is a deterministic error, never a panic.
    let mut rng = Rng(0x5eed_0019);
    for _ in 0..CASES {
        let ri = rng.u128_up_to_bits(128);
        let ro = rng.u128_up_to_bits(128);
        let ain = rng.u128_up_to_bits(128);
        if let Ok(o) = quote_swap_exact_in(ri, ro, ain) {
            assert!(o.amount_out < ro);
            assert_eq!(o.fees.lp_fee + o.fees.protocol_fee, o.fees.total_fee);
            assert!(o.fees.total_fee <= ain);
        }
        let _ = initial_liquidity(ri, ro);
        let _ = quote_add_liquidity(ri, ro, ain.max(1), rng.u128_up_to_bits(128), ain);
        let _ = quote_remove_liquidity(ri, ro, ain.max(1), rng.u128_up_to_bits(128));
    }
}
