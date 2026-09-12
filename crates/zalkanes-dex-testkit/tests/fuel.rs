//! §39 — deterministic fuel measurement and regression ceilings.

mod common;

use common::{Fixture, INIT_A, INIT_B};
use zalkanes_dex_core::encode::{
    factory_create_pool_args, factory_op, pool_add_liquidity_args, pool_op, pool_quote_args,
    pool_remove_liquidity_args, pool_swap_args,
};
use zalkanes_dex_core::types::AssetId;
use zalkanes_dex_testkit::FuelRecord;

/// Regression ceilings (units of the DEX-testkit fuel schedule). Raising
/// one requires an intentional review of what got more expensive.
const CEIL_CREATE_POOL: u64 = 20_000;
const CEIL_ADD: u64 = 4_000;
const CEIL_REMOVE: u64 = 4_000;
const CEIL_SWAP: u64 = 4_000;
const CEIL_QUOTE: u64 = 2_000;
const CEIL_DETAILS: u64 = 2_000;

struct Measured {
    log: Vec<FuelRecord>,
    create: u64,
    swap: u64,
    add: u64,
    remove: u64,
    quote: u64,
    details: u64,
}

fn run_measured() -> Measured {
    let (mut fx, pool) = Fixture::with_pool();
    let create = fx.chain.last_fuel().expect("create record").fuel_used;
    let (t0, t1, ..) = fx.canonical_amounts(INIT_A, INIT_B);
    fx.chain
        .call(
            fx.alice,
            pool,
            pool_op::SWAP_EXACT_IN,
            &pool_swap_args(0, 0),
            &[(t0, 100_000)],
        )
        .expect("swap");
    let swap = fx.chain.last_fuel().expect("swap record").fuel_used;
    fx.chain
        .call(
            fx.alice,
            pool,
            pool_op::ADD_LIQUIDITY,
            &pool_add_liquidity_args(0, 0),
            &[(t0, 100_000), (t1, 300_000)],
        )
        .expect("add");
    let add = fx.chain.last_fuel().expect("add record").fuel_used;
    fx.chain
        .call(
            fx.alice,
            pool,
            pool_op::REMOVE_LIQUIDITY,
            &pool_remove_liquidity_args(0, 0, 0),
            &[(Fixture::lp_asset(&pool), 50_000)],
        )
        .expect("remove");
    let remove = fx.chain.last_fuel().expect("remove record").fuel_used;
    let (quote_result, quote) =
        fx.chain
            .view_with_fuel(pool, pool_op::QUOTE_EXACT_IN, &pool_quote_args(&t0, 50_000));
    quote_result.expect("quote");
    let (details_result, details) = fx.chain.view_with_fuel(pool, pool_op::POOL_DETAILS, &[]);
    details_result.expect("details");
    Measured {
        log: fx.chain.fuel_log().to_vec(),
        create,
        swap,
        add,
        remove,
        quote,
        details,
    }
}

#[test]
fn fuel_deterministic_across_runs() {
    let run1 = run_measured();
    let run2 = run_measured();
    assert_eq!(
        run1.log, run2.log,
        "fuel log identical across pristine runs"
    );
    assert_eq!(run1.quote, run2.quote);
    assert_eq!(run1.details, run2.details);
}

#[test]
fn fuel_regression_ceilings() {
    let m = run_measured();
    assert!(m.create <= CEIL_CREATE_POOL, "create {}", m.create);
    assert!(m.swap <= CEIL_SWAP, "swap {}", m.swap);
    assert!(m.add <= CEIL_ADD, "add {}", m.add);
    assert!(m.remove <= CEIL_REMOVE, "remove {}", m.remove);
    assert!(m.quote <= CEIL_QUOTE, "quote {}", m.quote);
    assert!(m.details <= CEIL_DETAILS, "details {}", m.details);
    // Record the measured values in the assertion messages above; the
    // ceilings are the regression contract.
}

#[test]
fn fuel_swap_independent_of_pool_count() {
    // A swap must not iterate the registry: swap fuel with 1 pool equals
    // swap fuel with 40 pools.
    fn swap_fuel(extra_pools: u8) -> u64 {
        let (mut fx, pool) = Fixture::with_pool();
        for i in 0..extra_pools {
            let a = AssetId([i + 1; 32]);
            let b = AssetId([250u8.wrapping_sub(i); 32]);
            fx.chain.faucet(fx.alice, a, 10_000_000);
            fx.chain.faucet(fx.alice, b, 10_000_000);
            fx.chain
                .call(
                    fx.alice,
                    fx.factory,
                    factory_op::CREATE_POOL,
                    &factory_create_pool_args(&a, &b),
                    &[(a, 2_000_000), (b, 2_000_000)],
                )
                .expect("extra pool");
        }
        let (t0, ..) = fx.canonical_amounts(INIT_A, INIT_B);
        fx.chain
            .call(
                fx.alice,
                pool,
                pool_op::SWAP_EXACT_IN,
                &pool_swap_args(0, 0),
                &[(t0, 100_000)],
            )
            .expect("swap");
        fx.chain.last_fuel().expect("record").fuel_used
    }
    assert_eq!(
        swap_fuel(0),
        swap_fuel(40),
        "swap fuel is O(1) in pool count"
    );
}

#[test]
fn fuel_lookup_is_keyed_not_scanned() {
    // Pool lookup fuel with 1 pool == with 40 pools (keyed storage).
    fn lookup_fuel(extra_pools: u8) -> u64 {
        let (mut fx, _pool) = Fixture::with_pool();
        for i in 0..extra_pools {
            let a = AssetId([i + 1; 32]);
            let b = AssetId([250u8.wrapping_sub(i); 32]);
            fx.chain.faucet(fx.alice, a, 10_000_000);
            fx.chain.faucet(fx.alice, b, 10_000_000);
            fx.chain
                .call(
                    fx.alice,
                    fx.factory,
                    factory_op::CREATE_POOL,
                    &factory_create_pool_args(&a, &b),
                    &[(a, 2_000_000), (b, 2_000_000)],
                )
                .expect("extra pool");
        }
        let (_, fuel) = fx.chain.view_with_fuel(
            fx.factory,
            factory_op::GET_POOL,
            &zalkanes_dex_core::encode::factory_get_pool_args(&fx.za, &fx.zb),
        );
        fuel
    }
    assert_eq!(lookup_fuel(0), lookup_fuel(40));
}

#[test]
fn fuel_print_measurements() {
    // Reference values for the evidence report (run with --nocapture).
    let m = run_measured();
    println!(
        "fuel: create_pool={} swap={} add={} remove={} quote={} details={}",
        m.create, m.swap, m.add, m.remove, m.quote, m.details
    );
}
