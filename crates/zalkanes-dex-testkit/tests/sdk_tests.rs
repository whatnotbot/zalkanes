//! SDK layer (§22): intents → call plans → execution, and views.

mod common;

use common::{Fixture, INIT_A, INIT_B};
use zalkanes_dex_core::error::DexError;
use zalkanes_dex_core::types::Pair;
use zalkanes_dex_sdk::{
    build_add_liquidity, build_create_pool, build_remove_liquidity, build_swap_exact_in,
    AddLiquidityIntent, CreatePoolIntent, DexView, RemoveLiquidityIntent, SwapIntent,
};

#[test]
fn sdk_full_lifecycle_via_intents() {
    let mut fx = Fixture::new();

    // create pool
    let create = build_create_pool(&CreatePoolIntent {
        factory: fx.factory,
        token_a: fx.za,
        token_b: fx.zb,
        amount_a: INIT_A,
        amount_b: INIT_B,
    })
    .expect("plan");
    let out = fx.chain.execute_plan(fx.alice, &create).expect("create");
    assert_eq!(out.len(), 32);

    let view = DexView::new(&fx.chain, fx.factory);
    let pool = view
        .get_pool(fx.zb, fx.za)
        .expect("lookup")
        .expect("pool exists");
    assert_eq!(out, pool.0.to_vec());
    assert_eq!(view.pool_count().unwrap(), 1);
    assert_eq!(view.get_all_pools().unwrap(), vec![pool]);

    let details = view.pool_details(pool).expect("details");
    let pair = Pair::canonical(fx.za, fx.zb).unwrap();
    assert_eq!(details.token0, pair.token0);
    assert_eq!(details.token1, pair.token1);
    assert_eq!(
        view.get_reserves(pool).unwrap(),
        (details.reserve0, details.reserve1)
    );

    // quote: contract-evaluated == locally computed
    let contract_quote = view.quote_exact_in(pool, fx.za, 123_456).expect("quote");
    let local_quote = view
        .quote_exact_in_local(pool, fx.za, 123_456)
        .expect("local");
    assert_eq!(contract_quote, local_quote);

    // swap via intent, output must equal quote
    let swap = build_swap_exact_in(&SwapIntent {
        pool,
        incoming_asset: fx.za,
        incoming_amount: 123_456,
        min_amount_out: contract_quote.amount_out,
        expiry_height: 0,
    })
    .expect("swap plan");
    let before = fx.chain.balance(&fx.alice, &contract_quote.token_out);
    fx.chain.execute_plan(fx.alice, &swap).expect("swap");
    assert_eq!(
        fx.chain.balance(&fx.alice, &contract_quote.token_out),
        before + contract_quote.amount_out
    );

    // add liquidity via intent
    let view = DexView::new(&fx.chain, fx.factory);
    let details = view.pool_details(pool).expect("details");
    let add = build_add_liquidity(&AddLiquidityIntent {
        pool,
        token0: details.token0,
        token1: details.token1,
        desired0: details.reserve0 / 10,
        desired1: details.reserve1 / 7,
        min_lp_out: 1,
        expiry_height: 0,
    })
    .expect("add plan");
    fx.chain.execute_plan(fx.alice, &add).expect("add");

    // remove liquidity via intent
    let remove = build_remove_liquidity(&RemoveLiquidityIntent {
        pool,
        lp_amount: 100_000,
        min_amount0: 1,
        min_amount1: 1,
        expiry_height: 0,
    })
    .expect("remove plan");
    fx.chain.execute_plan(fx.alice, &remove).expect("remove");
}

#[test]
fn sdk_rejects_invalid_intents() {
    let fx = Fixture::new();
    assert_eq!(
        build_create_pool(&CreatePoolIntent {
            factory: fx.factory,
            token_a: fx.za,
            token_b: fx.za,
            amount_a: 1,
            amount_b: 1,
        }),
        Err(DexError::IdenticalAssets)
    );
    assert_eq!(
        build_swap_exact_in(&SwapIntent {
            pool: fx.factory,
            incoming_asset: fx.za,
            incoming_amount: 0,
            min_amount_out: 0,
            expiry_height: 0,
        }),
        Err(DexError::ZeroAmount)
    );
    let view = DexView::new(&fx.chain, fx.factory);
    assert_eq!(view.get_pool(fx.za, fx.zb).expect("lookup"), None);
}
