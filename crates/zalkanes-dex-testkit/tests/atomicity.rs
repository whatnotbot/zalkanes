//! §33 — failure atomicity matrix. For every failing mutating call the
//! state root, user asset state and pool asset state must be identical
//! before and after.

mod common;

use common::{Fixture, INIT_A, INIT_B};
use zalkanes_dex_core::encode::{
    factory_create_pool_args, factory_op, pool_add_liquidity_args, pool_op,
    pool_remove_liquidity_args, pool_swap_args,
};
use zalkanes_dex_core::error::DexError;
use zalkanes_dex_core::types::{AssetId, ContractId, Holder};
use zalkanes_dex_testkit::Call;

/// Execute one failing call and assert full atomicity.
fn assert_atomic_failure(
    fx: &mut Fixture,
    pool: Option<ContractId>,
    call: Call,
    expected: DexError,
) {
    let root_before = fx.chain.state_root();
    let user_za = fx.balance(&call.caller, &fx.za);
    let user_zb = fx.balance(&call.caller, &fx.zb);
    let (pool_za, pool_zb, user_lp) = match pool {
        Some(p) => (
            fx.balance(&Holder::Contract(p), &fx.za),
            fx.balance(&Holder::Contract(p), &fx.zb),
            fx.balance(&call.caller, &Fixture::lp_asset(&p)),
        ),
        None => (0, 0, 0),
    };

    let caller = call.caller;
    let err = fx
        .chain
        .call(caller, call.target, call.opcode, &call.input, &call.assets)
        .expect_err("call must fail");
    assert_eq!(err, expected, "expected failure code");

    // The failed call sealed an (empty) block; compare against the
    // pre-call state at the previous height.
    let height = fx.chain.height();
    assert_eq!(
        fx.chain.root_at(height - 1),
        root_before,
        "state root unchanged"
    );
    assert_eq!(
        fx.chain.root_at(height),
        root_before,
        "post-block root unchanged"
    );
    assert_eq!(fx.balance(&caller, &fx.za), user_za, "user ZA unchanged");
    assert_eq!(fx.balance(&caller, &fx.zb), user_zb, "user ZB unchanged");
    if let Some(p) = pool {
        assert_eq!(
            fx.balance(&Holder::Contract(p), &fx.za),
            pool_za,
            "pool ZA unchanged"
        );
        assert_eq!(
            fx.balance(&Holder::Contract(p), &fx.zb),
            pool_zb,
            "pool ZB unchanged"
        );
        assert_eq!(
            fx.balance(&caller, &Fixture::lp_asset(&p)),
            user_lp,
            "user LP unchanged"
        );
    }
}

#[test]
fn atomic_duplicate_pool() {
    let (mut fx, pool) = Fixture::with_pool();
    let call = fx.simple_call(
        fx.alice,
        fx.factory,
        factory_op::CREATE_POOL,
        factory_create_pool_args(&fx.zb, &fx.za),
        vec![(fx.za, INIT_A), (fx.zb, INIT_B)],
    );
    assert_atomic_failure(&mut fx, Some(pool), call, DexError::DuplicatePool);
}

#[test]
fn atomic_failed_initialize() {
    let mut fx = Fixture::new();
    let call = fx.simple_call(
        fx.alice,
        fx.factory,
        factory_op::CREATE_POOL,
        factory_create_pool_args(&fx.za, &fx.zb),
        vec![(fx.za, 900), (fx.zb, 900)],
    );
    assert_atomic_failure(&mut fx, None, call, DexError::InsufficientInitialLiquidity);
}

#[test]
fn atomic_min_lp() {
    let (mut fx, pool) = Fixture::with_pool();
    let call = fx.simple_call(
        fx.alice,
        pool,
        pool_op::ADD_LIQUIDITY,
        pool_add_liquidity_args(u128::MAX, 0),
        vec![(fx.za, 100_000), (fx.zb, 400_000)],
    );
    assert_atomic_failure(&mut fx, Some(pool), call, DexError::SlippageExceeded);
}

#[test]
fn atomic_min_output() {
    let (mut fx, pool) = Fixture::with_pool();
    let call = fx.simple_call(
        fx.alice,
        pool,
        pool_op::SWAP_EXACT_IN,
        pool_swap_args(u128::MAX, 0),
        vec![(fx.za, 100_000)],
    );
    assert_atomic_failure(&mut fx, Some(pool), call, DexError::SlippageExceeded);
}

#[test]
fn atomic_min_withdrawal() {
    let (mut fx, pool) = Fixture::with_pool();
    let call = fx.simple_call(
        fx.alice,
        pool,
        pool_op::REMOVE_LIQUIDITY,
        pool_remove_liquidity_args(u128::MAX, u128::MAX, 0),
        vec![(Fixture::lp_asset(&pool), 1_000_000)],
    );
    assert_atomic_failure(&mut fx, Some(pool), call, DexError::SlippageExceeded);
}

#[test]
fn atomic_expired_call() {
    let (mut fx, pool) = Fixture::with_pool();
    let expired = fx.chain.height();
    let call = fx.simple_call(
        fx.alice,
        pool,
        pool_op::SWAP_EXACT_IN,
        pool_swap_args(0, expired),
        vec![(fx.za, 100_000)],
    );
    assert_atomic_failure(&mut fx, Some(pool), call, DexError::Expired);
}

#[test]
fn atomic_wrong_asset() {
    let (mut fx, pool) = Fixture::with_pool();
    let stranger = AssetId([0x77; 32]);
    fx.chain.faucet(fx.alice, stranger, 1_000_000);
    let call = fx.simple_call(
        fx.alice,
        pool,
        pool_op::SWAP_EXACT_IN,
        pool_swap_args(0, 0),
        vec![(stranger, 100_000)],
    );
    assert_atomic_failure(&mut fx, Some(pool), call, DexError::InvalidAsset);
}

#[test]
fn atomic_overflow() {
    let (mut fx, pool) = Fixture::with_pool();
    // Push a swap whose amount_in * 9900 overflows u128.
    let huge = u128::MAX / 100;
    fx.chain.faucet(fx.alice, fx.za, huge);
    let call = fx.simple_call(
        fx.alice,
        pool,
        pool_op::SWAP_EXACT_IN,
        pool_swap_args(0, 0),
        vec![(fx.za, huge)],
    );
    assert_atomic_failure(&mut fx, Some(pool), call, DexError::ArithmeticOverflow);
}

#[test]
fn atomic_reentrancy() {
    let (mut fx, pool) = Fixture::with_pool();
    fx.chain.set_storage_raw(&pool, b"lock", &[1]);
    let root_locked = fx.chain.state_root();
    let call = fx.simple_call(
        fx.alice,
        pool,
        pool_op::SWAP_EXACT_IN,
        pool_swap_args(0, 0),
        vec![(fx.za, 100_000)],
    );
    let err = fx
        .chain
        .call(
            call.caller,
            call.target,
            call.opcode,
            &call.input,
            &call.assets,
        )
        .expect_err("locked");
    assert_eq!(err, DexError::Reentrancy);
    let height = fx.chain.height();
    assert_eq!(fx.chain.root_at(height), root_locked);
}

#[test]
fn atomic_out_of_fuel() {
    let (mut fx, pool) = Fixture::with_pool();
    fx.chain.set_next_fuel_limit(700);
    let call = fx.simple_call(
        fx.alice,
        pool,
        pool_op::SWAP_EXACT_IN,
        pool_swap_args(0, 0),
        vec![(fx.za, 100_000)],
    );
    assert_atomic_failure(&mut fx, Some(pool), call, DexError::OutOfFuel);
}

#[test]
fn atomic_malformed_calldata() {
    let (mut fx, pool) = Fixture::with_pool();
    let call = fx.simple_call(
        fx.alice,
        pool,
        pool_op::SWAP_EXACT_IN,
        vec![0xab; 7],
        vec![(fx.za, 100_000)],
    );
    assert_atomic_failure(&mut fx, Some(pool), call, DexError::InvalidArguments);
}
