//! POOL-001..035 — pool contract runtime tests (spec §31).

mod common;

use common::{Fixture, BOB_ZA, INIT_A, INIT_B};
use zalkanes_dex_core::encode::{
    decode_pool_details, decode_reserves, decode_swap_quote, pool_add_liquidity_args,
    pool_initialize_args, pool_op, pool_quote_args, pool_remove_liquidity_args, pool_swap_args,
};
use zalkanes_dex_core::error::DexError;
use zalkanes_dex_core::host::ContractSpawner;
use zalkanes_dex_core::math::{quote_add_liquidity, quote_remove_liquidity, quote_swap_exact_in};
use zalkanes_dex_core::types::{AssetId, Holder};
use zalkanes_dex_core::v0::MINIMUM_LIQUIDITY;
use zalkanes_dex_testkit::{account, ContractKind, DexChain};

fn gross_lp() -> u128 {
    // isqrt(INIT_A * INIT_B) = isqrt(10^14) = 10^7
    10_000_000
}

#[test]
fn pool_001_fresh_pool_uninitialized() {
    let mut chain = DexChain::new();
    let pool = chain.deploy(ContractKind::SubfrostPool);
    assert_eq!(
        chain.view(pool, pool_op::GET_RESERVES, &[]),
        Err(DexError::NotInitialized)
    );
    assert_eq!(
        chain.view(pool, pool_op::POOL_DETAILS, &[]),
        Err(DexError::NotInitialized)
    );
}

#[test]
fn pool_002_factory_initialization_succeeds() {
    let (fx, pool) = Fixture::with_pool();
    let details = decode_pool_details(&fx.chain.view(pool, pool_op::POOL_DETAILS, &[]).unwrap())
        .expect("details decode");
    assert_eq!(details.pool_id, pool);
    assert_eq!(details.factory_id, fx.factory);
}

#[test]
fn pool_003_second_initialization_fails() {
    let (mut fx, pool) = Fixture::with_pool();
    // Even a contract caller cannot re-initialize.
    let err = fx
        .chain
        .call(
            Holder::Contract(fx.factory),
            pool,
            pool_op::INITIALIZE,
            &[],
            &[],
        )
        .expect_err("second init");
    assert_eq!(err, DexError::AlreadyInitialized);
}

#[test]
fn pool_004_non_factory_initialize_fails() {
    let mut fx = Fixture::new();
    let pool = fx.chain.deploy(ContractKind::SubfrostPool);
    let (t0, t1, a0, a1) = fx.canonical_amounts(INIT_A, INIT_B);
    let args = pool_initialize_args(&t0, &t1, &fx.alice);
    let err = fx
        .chain
        .call(
            fx.alice, // external caller
            pool,
            pool_op::INITIALIZE,
            &args,
            &[(t0, a0), (t1, a1)],
        )
        .expect_err("external init");
    assert_eq!(err, DexError::UnauthorizedInitialize);
}

/// Directly initialize a fresh pool with a spoofed contract caller.
fn raw_init(
    fx: &mut Fixture,
    args: Vec<u8>,
    assets: Vec<(AssetId, u128)>,
) -> Result<Vec<u8>, DexError> {
    let pool = fx.chain.deploy(ContractKind::SubfrostPool);
    let spoofed = Holder::Contract(fx.factory);
    // Give the "factory" custody of whatever it is about to forward.
    for (asset, amount) in &assets {
        fx.chain.faucet(spoofed, *asset, *amount);
    }
    fx.chain
        .call(spoofed, pool, pool_op::INITIALIZE, &args, &assets)
}

#[test]
fn pool_005_wrong_tokens_fail() {
    let mut fx = Fixture::new();
    let (t0, t1, ..) = fx.canonical_amounts(INIT_A, INIT_B);
    let stranger = AssetId([0x55; 32]);
    let args = pool_initialize_args(&t0, &t1, &fx.alice);
    let err =
        raw_init(&mut fx, args, vec![(stranger, INIT_A), (t1, INIT_B)]).expect_err("wrong tokens");
    assert_eq!(err, DexError::InvalidAsset);
}

#[test]
fn pool_006_one_token_missing_fails() {
    let mut fx = Fixture::new();
    let (t0, t1, a0, _a1) = fx.canonical_amounts(INIT_A, INIT_B);
    let args = pool_initialize_args(&t0, &t1, &fx.alice);
    let err = raw_init(&mut fx, args, vec![(t0, a0)]).expect_err("one token");
    assert_eq!(err, DexError::InvalidIncomingAssets);
}

#[test]
fn pool_007_initial_lp_minted_correctly() {
    let (fx, pool) = Fixture::with_pool();
    let lp = Fixture::lp_asset(&pool);
    assert_eq!(fx.balance(&fx.alice, &lp), gross_lp() - MINIMUM_LIQUIDITY);
}

#[test]
fn pool_008_minimum_lp_permanently_locked() {
    let (fx, pool) = Fixture::with_pool();
    let details = decode_pool_details(&fx.chain.view(pool, pool_op::POOL_DETAILS, &[]).unwrap())
        .expect("details");
    let lp = Fixture::lp_asset(&pool);
    // Supply counts the locked minimum; no holder owns it (not even the pool).
    assert_eq!(details.total_lp_supply, gross_lp());
    assert_eq!(fx.balance(&fx.alice, &lp), gross_lp() - MINIMUM_LIQUIDITY);
    assert_eq!(fx.balance(&Holder::Contract(pool), &lp), 0);
}

#[test]
fn pool_009_reserves_correct_after_initialize() {
    let (fx, pool) = Fixture::with_pool();
    let (_, _, r0, r1) = fx.canonical_amounts(INIT_A, INIT_B);
    let reserves =
        decode_reserves(&fx.chain.view(pool, pool_op::GET_RESERVES, &[]).unwrap()).unwrap();
    assert_eq!(reserves, (r0, r1));
}

#[test]
fn pool_010_lp_asset_equals_pool_identity() {
    let (fx, pool) = Fixture::with_pool();
    let lp = Fixture::lp_asset(&pool);
    assert_eq!(lp.0, pool.0, "LP AssetId == pool ContractId bytes");
    assert!(fx.balance(&fx.alice, &lp) > 0);
}

#[test]
fn pool_011_add_exact_ratio_succeeds() {
    let (mut fx, pool) = Fixture::with_pool();
    let (t0, t1, r0, r1) = fx.canonical_amounts(INIT_A, INIT_B);
    let (d0, d1) = (r0 / 10, r1 / 10);
    let expected = quote_add_liquidity(r0, r1, gross_lp(), d0, d1).unwrap();
    let lp_before = fx.balance(&fx.alice, &Fixture::lp_asset(&pool));
    let out = fx
        .chain
        .call(
            fx.alice,
            pool,
            pool_op::ADD_LIQUIDITY,
            &pool_add_liquidity_args(expected.lp_minted, 0),
            &[(t0, d0), (t1, d1)],
        )
        .expect("add");
    assert_eq!(out.len(), 80);
    assert_eq!(
        fx.balance(&fx.alice, &Fixture::lp_asset(&pool)),
        lp_before + expected.lp_minted
    );
    assert_eq!((expected.refund0, expected.refund1), (0, 0));
}

#[test]
fn pool_012_add_token0_excess_refunded_exactly() {
    let (mut fx, pool) = Fixture::with_pool();
    let (t0, t1, r0, r1) = fx.canonical_amounts(INIT_A, INIT_B);
    // Excess on token0: desired0 over ratio.
    let (d0, d1) = (r0 / 5, r1 / 10);
    let expected = quote_add_liquidity(r0, r1, gross_lp(), d0, d1).unwrap();
    assert!(expected.refund0 > 0);
    let bal0_before = fx.balance(&fx.alice, &t0);
    let bal1_before = fx.balance(&fx.alice, &t1);
    fx.chain
        .call(
            fx.alice,
            pool,
            pool_op::ADD_LIQUIDITY,
            &pool_add_liquidity_args(0, 0),
            &[(t0, d0), (t1, d1)],
        )
        .expect("add");
    assert_eq!(
        fx.balance(&fx.alice, &t0),
        bal0_before - expected.accepted0,
        "refund0 returned exactly"
    );
    assert_eq!(fx.balance(&fx.alice, &t1), bal1_before - expected.accepted1);
}

#[test]
fn pool_013_add_token1_excess_refunded_exactly() {
    let (mut fx, pool) = Fixture::with_pool();
    let (t0, t1, r0, r1) = fx.canonical_amounts(INIT_A, INIT_B);
    let (d0, d1) = (r0 / 10, r1 / 5);
    let expected = quote_add_liquidity(r0, r1, gross_lp(), d0, d1).unwrap();
    assert!(expected.refund1 > 0);
    let bal1_before = fx.balance(&fx.alice, &t1);
    fx.chain
        .call(
            fx.alice,
            pool,
            pool_op::ADD_LIQUIDITY,
            &pool_add_liquidity_args(0, 0),
            &[(t0, d0), (t1, d1)],
        )
        .expect("add");
    assert_eq!(fx.balance(&fx.alice, &t1), bal1_before - expected.accepted1);
}

#[test]
fn pool_014_min_lp_out_protects_caller() {
    let (mut fx, pool) = Fixture::with_pool();
    let (t0, t1, r0, r1) = fx.canonical_amounts(INIT_A, INIT_B);
    let (d0, d1) = (r0 / 10, r1 / 10);
    let expected = quote_add_liquidity(r0, r1, gross_lp(), d0, d1).unwrap();
    let err = fx
        .chain
        .call(
            fx.alice,
            pool,
            pool_op::ADD_LIQUIDITY,
            &pool_add_liquidity_args(expected.lp_minted + 1, 0),
            &[(t0, d0), (t1, d1)],
        )
        .expect_err("min lp");
    assert_eq!(err, DexError::SlippageExceeded);
}

#[test]
fn pool_015_expired_add_fails() {
    let (mut fx, pool) = Fixture::with_pool();
    let (t0, t1, r0, r1) = fx.canonical_amounts(INIT_A, INIT_B);
    let expired = fx.chain.height(); // next call executes at height+1
    let err = fx
        .chain
        .call(
            fx.alice,
            pool,
            pool_op::ADD_LIQUIDITY,
            &pool_add_liquidity_args(0, expired),
            &[(t0, r0 / 10), (t1, r1 / 10)],
        )
        .expect_err("expired");
    assert_eq!(err, DexError::Expired);
}

#[test]
fn pool_016_remove_succeeds() {
    let (mut fx, pool) = Fixture::with_pool();
    let (t0, t1, r0, r1) = fx.canonical_amounts(INIT_A, INIT_B);
    let lp = Fixture::lp_asset(&pool);
    let burn = 1_000_000u128;
    let (e0, e1) = quote_remove_liquidity(r0, r1, gross_lp(), burn).unwrap();
    let bal0 = fx.balance(&fx.alice, &t0);
    let bal1 = fx.balance(&fx.alice, &t1);
    let out = fx
        .chain
        .call(
            fx.alice,
            pool,
            pool_op::REMOVE_LIQUIDITY,
            &pool_remove_liquidity_args(e0, e1, 0),
            &[(lp, burn)],
        )
        .expect("remove");
    assert_eq!(out.len(), 32);
    assert_eq!(fx.balance(&fx.alice, &t0), bal0 + e0);
    assert_eq!(fx.balance(&fx.alice, &t1), bal1 + e1);
}

#[test]
fn pool_017_018_minimum_outputs_protect_caller() {
    let (mut fx, pool) = Fixture::with_pool();
    let (_, _, r0, r1) = fx.canonical_amounts(INIT_A, INIT_B);
    let lp = Fixture::lp_asset(&pool);
    let burn = 1_000_000u128;
    let (e0, e1) = quote_remove_liquidity(r0, r1, gross_lp(), burn).unwrap();
    for (min0, min1) in [(e0 + 1, e1), (e0, e1 + 1)] {
        let err = fx
            .chain
            .call(
                fx.alice,
                pool,
                pool_op::REMOVE_LIQUIDITY,
                &pool_remove_liquidity_args(min0, min1, 0),
                &[(lp, burn)],
            )
            .expect_err("min out");
        assert_eq!(err, DexError::SlippageExceeded);
    }
}

#[test]
fn pool_019_expired_remove_fails() {
    let (mut fx, pool) = Fixture::with_pool();
    let lp = Fixture::lp_asset(&pool);
    let expired = fx.chain.height();
    let err = fx
        .chain
        .call(
            fx.alice,
            pool,
            pool_op::REMOVE_LIQUIDITY,
            &pool_remove_liquidity_args(0, 0, expired),
            &[(lp, 1_000)],
        )
        .expect_err("expired");
    assert_eq!(err, DexError::Expired);
}

#[test]
fn pool_020_burn_greater_than_balance_fails() {
    let (mut fx, pool) = Fixture::with_pool();
    let lp = Fixture::lp_asset(&pool);
    let held = fx.balance(&fx.alice, &lp);
    let root_before = fx.chain.state_root();
    let err = fx
        .chain
        .call(
            fx.alice,
            pool,
            pool_op::REMOVE_LIQUIDITY,
            &pool_remove_liquidity_args(0, 0, 0),
            &[(lp, held + 1)],
        )
        .expect_err("over-burn");
    assert_eq!(err, DexError::InsufficientLiquidity);
    // Height advanced (block sealed) but no state mutation beyond that.
    fx.chain.rollback_to(fx.chain.height() - 1);
    assert_eq!(fx.chain.state_root(), root_before);
}

#[test]
fn pool_021_swap_a_to_b_succeeds() {
    let (mut fx, pool) = Fixture::with_pool();
    let (t0, t1, r0, r1) = fx.canonical_amounts(INIT_A, INIT_B);
    let amount_in = 100_000u128;
    let expected = quote_swap_exact_in(r0, r1, amount_in).unwrap();
    let bal_out_before = fx.balance(&fx.alice, &t1);
    fx.chain
        .call(
            fx.alice,
            pool,
            pool_op::SWAP_EXACT_IN,
            &pool_swap_args(expected.amount_out, 0),
            &[(t0, amount_in)],
        )
        .expect("swap 0->1");
    assert_eq!(
        fx.balance(&fx.alice, &t1),
        bal_out_before + expected.amount_out
    );
    let reserves =
        decode_reserves(&fx.chain.view(pool, pool_op::GET_RESERVES, &[]).unwrap()).unwrap();
    assert_eq!(
        reserves,
        (
            r0 + amount_in - expected.fees.protocol_fee,
            r1 - expected.amount_out
        )
    );
}

#[test]
fn pool_022_swap_b_to_a_succeeds() {
    let (mut fx, pool) = Fixture::with_pool();
    let (t0, t1, r0, r1) = fx.canonical_amounts(INIT_A, INIT_B);
    let amount_in = 250_000u128;
    let expected = quote_swap_exact_in(r1, r0, amount_in).unwrap();
    let bal_out_before = fx.balance(&fx.alice, &t0);
    fx.chain
        .call(
            fx.alice,
            pool,
            pool_op::SWAP_EXACT_IN,
            &pool_swap_args(expected.amount_out, 0),
            &[(t1, amount_in)],
        )
        .expect("swap 1->0");
    assert_eq!(
        fx.balance(&fx.alice, &t0),
        bal_out_before + expected.amount_out
    );
    let details =
        decode_pool_details(&fx.chain.view(pool, pool_op::POOL_DETAILS, &[]).unwrap()).unwrap();
    assert_eq!(details.protocol_fees1, expected.fees.protocol_fee);
    assert_eq!(details.protocol_fees0, 0);
}

#[test]
fn pool_023_min_output_protects_caller() {
    let (mut fx, pool) = Fixture::with_pool();
    let (t0, _, r0, r1) = fx.canonical_amounts(INIT_A, INIT_B);
    let amount_in = 100_000u128;
    let expected = quote_swap_exact_in(r0, r1, amount_in).unwrap();
    let err = fx
        .chain
        .call(
            fx.alice,
            pool,
            pool_op::SWAP_EXACT_IN,
            &pool_swap_args(expected.amount_out + 1, 0),
            &[(t0, amount_in)],
        )
        .expect_err("slippage");
    assert_eq!(err, DexError::SlippageExceeded);
}

#[test]
fn pool_024_expired_swap_fails() {
    let (mut fx, pool) = Fixture::with_pool();
    let (t0, ..) = fx.canonical_amounts(INIT_A, INIT_B);
    let expired = fx.chain.height();
    let err = fx
        .chain
        .call(
            fx.alice,
            pool,
            pool_op::SWAP_EXACT_IN,
            &pool_swap_args(0, expired),
            &[(t0, 100_000)],
        )
        .expect_err("expired");
    assert_eq!(err, DexError::Expired);
}

#[test]
fn pool_025_unrelated_token_fails() {
    let (mut fx, pool) = Fixture::with_pool();
    let stranger_contract = fx.chain.deploy(ContractKind::TestToken);
    let stranger = AssetId::of_contract(&stranger_contract);
    fx.chain.faucet(fx.alice, stranger, 1_000_000);
    let err = fx
        .chain
        .call(
            fx.alice,
            pool,
            pool_op::SWAP_EXACT_IN,
            &pool_swap_args(0, 0),
            &[(stranger, 100_000)],
        )
        .expect_err("unrelated");
    assert_eq!(err, DexError::InvalidAsset);
}

#[test]
fn pool_026_both_tokens_incoming_fails() {
    let (mut fx, pool) = Fixture::with_pool();
    let (t0, t1, ..) = fx.canonical_amounts(INIT_A, INIT_B);
    let err = fx
        .chain
        .call(
            fx.alice,
            pool,
            pool_op::SWAP_EXACT_IN,
            &pool_swap_args(0, 0),
            &[(t0, 100_000), (t1, 100_000)],
        )
        .expect_err("both");
    assert_eq!(err, DexError::InvalidIncomingAssets);
}

#[test]
fn pool_027_no_tokens_incoming_fails() {
    let (mut fx, pool) = Fixture::with_pool();
    let err = fx
        .chain
        .call(
            fx.alice,
            pool,
            pool_op::SWAP_EXACT_IN,
            &pool_swap_args(0, 0),
            &[],
        )
        .expect_err("none");
    assert_eq!(err, DexError::InvalidIncomingAssets);
}

#[test]
fn pool_028_zero_value_input_fails() {
    let (mut fx, pool) = Fixture::with_pool();
    let (t0, ..) = fx.canonical_amounts(INIT_A, INIT_B);
    let err = fx
        .chain
        .call(
            fx.alice,
            pool,
            pool_op::SWAP_EXACT_IN,
            &pool_swap_args(0, 0),
            &[(t0, 0)],
        )
        .expect_err("zero");
    assert_eq!(err, DexError::ZeroAmount);
}

#[test]
fn pool_029_protocol_fee_accounting_correct() {
    let (mut fx, pool) = Fixture::with_pool();
    let (t0, _, r0, r1) = fx.canonical_amounts(INIT_A, INIT_B);
    let mut expected_pfees0 = 0u128;
    let (mut cur0, mut cur1) = (r0, r1);
    for amount_in in [100_000u128, 333_333, 1_234_567] {
        let q = quote_swap_exact_in(cur0, cur1, amount_in).unwrap();
        fx.chain
            .call(
                fx.alice,
                pool,
                pool_op::SWAP_EXACT_IN,
                &pool_swap_args(0, 0),
                &[(t0, amount_in)],
            )
            .expect("swap");
        expected_pfees0 += q.fees.protocol_fee;
        cur0 = cur0 + amount_in - q.fees.protocol_fee;
        cur1 -= q.amount_out;
    }
    let details =
        decode_pool_details(&fx.chain.view(pool, pool_op::POOL_DETAILS, &[]).unwrap()).unwrap();
    assert_eq!(details.protocol_fees0, expected_pfees0);
    assert_eq!(details.reserve0, cur0);
    assert_eq!(details.reserve1, cur1);
    // Physical custody covers reserves + protocol fees exactly.
    let (t0_asset, t1_asset, ..) = fx.canonical_amounts(INIT_A, INIT_B);
    assert_eq!(
        fx.balance(&Holder::Contract(pool), &t0_asset),
        details.reserve0 + details.protocol_fees0
    );
    assert_eq!(
        fx.balance(&Holder::Contract(pool), &t1_asset),
        details.reserve1 + details.protocol_fees1
    );
}

#[test]
fn pool_030_lp_fee_accrues_to_lp_economics() {
    // After swaps, burning the same LP amount must return strictly more
    // token value product than before (LP fee stayed in reserves).
    let (mut fx, pool) = Fixture::with_pool();
    let (t0, ..) = fx.canonical_amounts(INIT_A, INIT_B);
    let lp = Fixture::lp_asset(&pool);
    let burn = 1_000_000u128;
    let (before0, before1) =
        quote_remove_liquidity(INIT_A.min(INIT_B), INIT_A.max(INIT_B), gross_lp(), burn).unwrap();
    // A round of swap volume.
    for _ in 0..5 {
        fx.chain
            .call(
                fx.alice,
                pool,
                pool_op::SWAP_EXACT_IN,
                &pool_swap_args(0, 0),
                &[(t0, 500_000)],
            )
            .expect("swap");
    }
    let details =
        decode_pool_details(&fx.chain.view(pool, pool_op::POOL_DETAILS, &[]).unwrap()).unwrap();
    let (after0, after1) = quote_remove_liquidity(
        details.reserve0,
        details.reserve1,
        details.total_lp_supply,
        burn,
    )
    .unwrap();
    // Constant-product value comparison: k share must not shrink.
    let before_k = before0 * before1;
    let after_k = after0 * after1;
    assert!(
        after_k >= before_k,
        "LP share value decreased: {before_k} -> {after_k}"
    );
    let _ = lp;
}

#[test]
fn pool_031_reentrancy_fails() {
    let (mut fx, pool) = Fixture::with_pool();
    let (t0, ..) = fx.canonical_amounts(INIT_A, INIT_B);
    // Model a mid-execution state: the lock is held.
    fx.chain.set_storage_raw(&pool, b"lock", &[1]);
    for (opcode, input, assets) in [
        (
            pool_op::SWAP_EXACT_IN,
            pool_swap_args(0, 0),
            vec![(t0, 100_000u128)],
        ),
        (
            pool_op::ADD_LIQUIDITY,
            pool_add_liquidity_args(0, 0),
            vec![(t0, 100_000u128)],
        ),
        (
            pool_op::REMOVE_LIQUIDITY,
            pool_remove_liquidity_args(0, 0, 0),
            vec![(Fixture::lp_asset(&pool), 1_000u128)],
        ),
    ] {
        let err = fx
            .chain
            .call(fx.alice, pool, opcode, &input, &assets)
            .expect_err("locked");
        assert_eq!(err, DexError::Reentrancy);
    }
}

#[test]
fn pool_032_malformed_calldata_fails() {
    let (mut fx, pool) = Fixture::with_pool();
    let (t0, ..) = fx.canonical_amounts(INIT_A, INIT_B);
    for input in [
        vec![],
        vec![0u8; 3],
        vec![0u8; 19],
        vec![0u8; 21],
        vec![0u8; 200],
    ] {
        let err = fx
            .chain
            .call(
                fx.alice,
                pool,
                pool_op::SWAP_EXACT_IN,
                &input,
                &[(t0, 100_000)],
            )
            .expect_err("malformed");
        assert_eq!(err, DexError::InvalidArguments);
    }
}

#[test]
fn pool_033_unknown_opcode_fails() {
    let (mut fx, pool) = Fixture::with_pool();
    for opcode in [4u16, 5, 42, 96, 101, 998, 1000, u16::MAX] {
        let err = fx
            .chain
            .call(fx.alice, pool, opcode, &[], &[])
            .expect_err("unknown opcode");
        assert_eq!(err, DexError::InvalidOpcode);
    }
    // Opcode 98 is reserved but unsupported (no fake TWAP).
    assert_eq!(
        fx.chain.view(pool, pool_op::GET_PRICE_CUMULATIVE, &[]),
        Err(DexError::Unsupported)
    );
}

#[test]
fn pool_034_out_of_fuel_fails_atomically() {
    let (mut fx, pool) = Fixture::with_pool();
    let (t0, ..) = fx.canonical_amounts(INIT_A, INIT_B);
    let root_before = fx.chain.state_root();
    fx.chain.set_next_fuel_limit(1_300); // enough to start, not to finish
    let err = fx
        .chain
        .call(
            fx.alice,
            pool,
            pool_op::SWAP_EXACT_IN,
            &pool_swap_args(0, 0),
            &[(t0, 100_000)],
        )
        .expect_err("fuel");
    assert_eq!(err, DexError::OutOfFuel);
    fx.chain.rollback_to(fx.chain.height() - 1);
    assert_eq!(fx.chain.state_root(), root_before);
}

#[test]
fn pool_035_mid_write_failure_is_atomic() {
    // Fuel exhausts midway through the storage-write sequence of a swap;
    // nothing may persist.
    let (mut fx, pool) = Fixture::with_pool();
    let (t0, t1, r0, r1) = fx.canonical_amounts(INIT_A, INIT_B);
    let lp_supply_before =
        decode_pool_details(&fx.chain.view(pool, pool_op::POOL_DETAILS, &[]).unwrap()).unwrap();
    let alice_t0 = fx.balance(&fx.alice, &t0);
    let alice_t1 = fx.balance(&fx.alice, &t1);
    for limit in [1_350u64, 1_400, 1_450, 1_500, 1_550] {
        fx.chain.set_next_fuel_limit(limit);
        let result = fx.chain.call(
            fx.alice,
            pool,
            pool_op::SWAP_EXACT_IN,
            &pool_swap_args(0, 0),
            &[(t0, 100_000)],
        );
        if result.is_ok() {
            continue; // enough fuel — fine
        }
        let details =
            decode_pool_details(&fx.chain.view(pool, pool_op::POOL_DETAILS, &[]).unwrap()).unwrap();
        assert_eq!(
            (details.reserve0, details.reserve1),
            (r0, r1),
            "limit {limit}"
        );
        assert_eq!(details.total_lp_supply, lp_supply_before.total_lp_supply);
        assert_eq!(fx.balance(&fx.alice, &t0), alice_t0, "limit {limit}");
        assert_eq!(fx.balance(&fx.alice, &t1), alice_t1, "limit {limit}");
    }
}

#[test]
fn pool_quote_matches_execution_exactly() {
    // AC-POOL-10: quote view == actual swap result on unchanged state.
    let (mut fx, pool) = Fixture::with_pool();
    let (t0, t1, ..) = fx.canonical_amounts(INIT_A, INIT_B);
    let amount_in = BOB_ZA / 100;
    let quote_bytes = fx
        .chain
        .view(
            pool,
            pool_op::QUOTE_EXACT_IN,
            &pool_quote_args(&t0, amount_in),
        )
        .expect("quote");
    let quote = decode_swap_quote(&quote_bytes).expect("decode quote");
    assert_eq!(quote.token_out, t1);
    let bal_before = fx.balance(&fx.bob, &t1);
    // Bob holds ZA which may be token0 or token1; use whichever side.
    let bob_asset = if fx.za == t0 { t0 } else { t1 };
    if bob_asset == t0 {
        fx.chain
            .call(
                fx.bob,
                pool,
                pool_op::SWAP_EXACT_IN,
                &pool_swap_args(quote.amount_out, 0),
                &[(t0, amount_in)],
            )
            .expect("swap");
        assert_eq!(fx.balance(&fx.bob, &t1), bal_before + quote.amount_out);
    } else {
        // Quote the actual direction Bob can trade.
        let quote2 = decode_swap_quote(
            &fx.chain
                .view(
                    pool,
                    pool_op::QUOTE_EXACT_IN,
                    &pool_quote_args(&t1, amount_in),
                )
                .expect("quote2"),
        )
        .expect("decode");
        let before = fx.balance(&fx.bob, &t0);
        fx.chain
            .call(
                fx.bob,
                pool,
                pool_op::SWAP_EXACT_IN,
                &pool_swap_args(quote2.amount_out, 0),
                &[(t1, amount_in)],
            )
            .expect("swap");
        assert_eq!(fx.balance(&fx.bob, &t0), before + quote2.amount_out);
    }
}

#[test]
fn pool_name_and_views_canonical() {
    let (fx, pool) = Fixture::with_pool();
    let name = fx.chain.view(pool, pool_op::GET_NAME, &[]).expect("name");
    // Constituent order follows the canonical pair order.
    let expected = if fx.za < fx.zb {
        "Zalkanes Test Asset A / Zalkanes Test Asset B LP"
    } else {
        "Zalkanes Test Asset B / Zalkanes Test Asset A LP"
    };
    assert_eq!(String::from_utf8(name).expect("utf8"), expected);
}

#[test]
fn pool_spawner_trait_is_narrow_and_atomic() {
    // Direct use of the ContractSpawner abstraction (spec §20): spawning
    // with a bad init reverts the spawn itself.
    let mut fx = Fixture::new();
    let spoofed = Holder::Contract(fx.factory);
    fx.chain.faucet(spoofed, fx.za, 10);
    fx.chain.faucet(spoofed, fx.zb, 10);
    let root_before = fx.chain.state_root();
    // Too little liquidity -> pool init fails -> spawn must not persist.
    let (t0, t1, ..) = fx.canonical_amounts(10, 10);
    let result = fx.chain.call(
        spoofed,
        fx.factory,
        zalkanes_dex_core::encode::factory_op::CREATE_POOL,
        &zalkanes_dex_core::encode::factory_create_pool_args(&t0, &t1),
        &[(t0, 10), (t1, 10)],
    );
    assert_eq!(result, Err(DexError::InsufficientInitialLiquidity));
    fx.chain.rollback_to(fx.chain.height() - 1);
    assert_eq!(fx.chain.state_root(), root_before);
    // The trait itself stays object-safe and narrow.
    fn _assert_narrow(spawner: &mut dyn ContractSpawner) {
        let _ = spawner;
    }
}

#[test]
fn pool_name_falls_back_for_nameless_assets() {
    // Assets whose contracts have no name (uninitialized token) get the
    // hex-prefix fallback, mirroring upstream's "{block},{tx}" fallback.
    let mut fx = Fixture::new();
    let raw_a = fx.chain.deploy(ContractKind::TestToken); // never initialized
    let raw_b = fx.chain.deploy(ContractKind::TestToken);
    let (ua, ub) = (AssetId::of_contract(&raw_a), AssetId::of_contract(&raw_b));
    let carol = Holder::External(account("carol"));
    fx.chain.faucet(carol, ua, 10_000_000);
    fx.chain.faucet(carol, ub, 10_000_000);
    let out = fx
        .chain
        .call(
            carol,
            fx.factory,
            zalkanes_dex_core::encode::factory_op::CREATE_POOL,
            &zalkanes_dex_core::encode::factory_create_pool_args(&ua, &ub),
            &[(ua, 5_000_000), (ub, 5_000_000)],
        )
        .expect("create");
    let mut pool_id = [0u8; 32];
    pool_id.copy_from_slice(&out);
    let name = fx
        .chain
        .view(
            zalkanes_dex_core::types::ContractId(pool_id),
            pool_op::GET_NAME,
            &[],
        )
        .expect("name");
    let name = String::from_utf8(name).expect("utf8");
    assert!(name.ends_with(" LP"));
    assert!(name.contains(" / "));
}
