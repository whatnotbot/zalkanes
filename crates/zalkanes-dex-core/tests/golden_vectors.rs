//! Golden-vector replay (spec §28).
//!
//! Expected values come from the independent Python big-integer reference
//! (`tools/gen_subfrost_vectors.py`), never from the Rust code under test.
//! Canonical artifact: `test-vectors/subfrost_amm_v0_vectors.json`.

#[path = "golden/data.rs"]
mod data;

use data::{Expected, VECTORS};
use zalkanes_dex_core::error::DexError;
use zalkanes_dex_core::math::{
    initial_liquidity, quote_add_liquidity, quote_remove_liquidity, quote_swap_exact_in,
};
use zalkanes_dex_core::v0::MINIMUM_LIQUIDITY;

#[test]
fn golden_vector_count_meets_spec() {
    assert!(VECTORS.len() >= 50, "need at least 50 vectors");
    let count = |p: &str| VECTORS.iter().filter(|v| v.id.starts_with(p)).count();
    assert!(count("INIT-") >= 10);
    assert!(count("ADD-") >= 10);
    assert!(count("REM-") >= 10);
    assert!(count("SWAP-") >= 15);
    assert!(count("FAIL-") >= 5);
}

fn expect_err(id: &str, code: u32, got: DexError) {
    let expected = DexError::from_code(code).expect("known error code");
    assert_eq!(expected, got, "{id}: wrong error");
}

#[test]
fn golden_vectors_replay_exactly() {
    for v in VECTORS {
        let [a, b, c, d, _e] = v.args;
        match (v.op, &v.expected) {
            (
                "initialize",
                Expected::Initialize {
                    provider_lp,
                    locked_lp,
                    total_lp_supply,
                },
            ) => {
                let (provider, gross) = initial_liquidity(a, b)
                    .unwrap_or_else(|err| panic!("{}: unexpected error {err}", v.id));
                assert_eq!(provider, *provider_lp, "{}", v.id);
                assert_eq!(gross, *total_lp_supply, "{}", v.id);
                assert_eq!(MINIMUM_LIQUIDITY, *locked_lp, "{}", v.id);
            }
            ("initialize", Expected::Error(code)) => {
                expect_err(v.id, *code, initial_liquidity(a, b).expect_err(v.id));
            }
            (
                "add_liquidity",
                Expected::Add {
                    accepted0,
                    accepted1,
                    refund0,
                    refund1,
                    lp_minted,
                },
            ) => {
                let o = quote_add_liquidity(a, b, c, d, v.args[4])
                    .unwrap_or_else(|err| panic!("{}: unexpected error {err}", v.id));
                assert_eq!(o.accepted0, *accepted0, "{}", v.id);
                assert_eq!(o.accepted1, *accepted1, "{}", v.id);
                assert_eq!(o.refund0, *refund0, "{}", v.id);
                assert_eq!(o.refund1, *refund1, "{}", v.id);
                assert_eq!(o.lp_minted, *lp_minted, "{}", v.id);
            }
            ("add_liquidity", Expected::Error(code)) => {
                expect_err(
                    v.id,
                    *code,
                    quote_add_liquidity(a, b, c, d, v.args[4]).expect_err(v.id),
                );
            }
            ("remove_liquidity", Expected::Remove { amount0, amount1 }) => {
                let (a0, a1) = quote_remove_liquidity(a, b, c, d)
                    .unwrap_or_else(|err| panic!("{}: unexpected error {err}", v.id));
                assert_eq!((a0, a1), (*amount0, *amount1), "{}", v.id);
            }
            ("remove_liquidity", Expected::Error(code)) => {
                expect_err(
                    v.id,
                    *code,
                    quote_remove_liquidity(a, b, c, d).expect_err(v.id),
                );
            }
            (
                "swap_exact_in",
                Expected::Swap {
                    amount_out,
                    total_fee,
                    lp_fee,
                    protocol_fee,
                },
            ) => {
                let o = quote_swap_exact_in(a, b, c)
                    .unwrap_or_else(|err| panic!("{}: unexpected error {err}", v.id));
                assert_eq!(o.amount_out, *amount_out, "{}", v.id);
                assert_eq!(o.fees.total_fee, *total_fee, "{}", v.id);
                assert_eq!(o.fees.lp_fee, *lp_fee, "{}", v.id);
                assert_eq!(o.fees.protocol_fee, *protocol_fee, "{}", v.id);
            }
            ("swap_exact_in", Expected::Error(code)) => {
                expect_err(v.id, *code, quote_swap_exact_in(a, b, c).expect_err(v.id));
            }
            (op, _) => panic!("{}: unknown op/expectation pairing {op}", v.id),
        }
    }
}
