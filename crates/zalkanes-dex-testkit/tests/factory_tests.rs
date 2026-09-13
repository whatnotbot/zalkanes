//! FACTORY-001..012 — factory contract runtime tests (spec §32).

mod common;

use common::{Fixture, INIT_A, INIT_B};
use zalkanes_dex_core::encode::{
    decode_pool_list, factory_create_pool_args, factory_get_pool_args, factory_op,
    token_initialize_args, token_mint_args, token_op,
};
use zalkanes_dex_core::error::DexError;
use zalkanes_dex_core::types::{AssetId, ContractId, Holder};
use zalkanes_dex_testkit::{account, ContractKind, DexChain};

fn pool_id_from(out: Vec<u8>) -> ContractId {
    assert_eq!(out.len(), 32);
    let mut id = [0u8; 32];
    id.copy_from_slice(&out);
    ContractId(id)
}

#[test]
fn factory_001_create_pool() {
    let (fx, pool) = Fixture::with_pool();
    assert!(fx.chain.get_storage_raw(&pool, b"init").is_some());
}

#[test]
fn factory_002_003_lookup_both_orders_find_same_pool() {
    let (fx, pool) = Fixture::with_pool();
    let ab = fx
        .chain
        .view(
            fx.factory,
            factory_op::GET_POOL,
            &factory_get_pool_args(&fx.za, &fx.zb),
        )
        .expect("lookup A/B");
    let ba = fx
        .chain
        .view(
            fx.factory,
            factory_op::GET_POOL,
            &factory_get_pool_args(&fx.zb, &fx.za),
        )
        .expect("lookup B/A");
    assert_eq!(ab, pool.0.to_vec());
    assert_eq!(ba, pool.0.to_vec());
}

#[test]
fn factory_004_duplicate_pool_rejected() {
    let (mut fx, _pool) = Fixture::with_pool();
    // B/A after A/B must be rejected as the same canonical pair.
    let err = fx
        .chain
        .call(
            fx.alice,
            fx.factory,
            factory_op::CREATE_POOL,
            &factory_create_pool_args(&fx.zb, &fx.za),
            &[(fx.za, INIT_A), (fx.zb, INIT_B)],
        )
        .expect_err("duplicate");
    assert_eq!(err, DexError::DuplicatePool);
}

#[test]
fn factory_005_identical_assets_rejected() {
    let mut fx = Fixture::new();
    let err = fx
        .chain
        .call(
            fx.alice,
            fx.factory,
            factory_op::CREATE_POOL,
            &factory_create_pool_args(&fx.za, &fx.za),
            &[(fx.za, INIT_A)],
        )
        .expect_err("identical");
    assert_eq!(err, DexError::IdenticalAssets);
}

#[test]
fn factory_006_pool_count_increments_once() {
    let (fx, _pool) = Fixture::with_pool();
    let count = fx
        .chain
        .view(fx.factory, factory_op::POOL_COUNT, &[])
        .expect("count");
    assert_eq!(count, 1u128.to_be_bytes().to_vec());
}

#[test]
fn factory_007_pool_list_deterministic() {
    let (fx, pool) = Fixture::with_pool();
    let list = decode_pool_list(
        &fx.chain
            .view(fx.factory, factory_op::GET_ALL_POOLS, &[])
            .expect("list"),
    )
    .expect("decode");
    assert_eq!(list, vec![pool]);
}

#[test]
fn factory_008_failed_pool_init_does_not_register() {
    let mut fx = Fixture::new();
    // Insufficient initial liquidity: pool init fails inside create_pool.
    let err = fx.create_pool(500, 500).expect_err("too small");
    assert_eq!(err, DexError::InsufficientInitialLiquidity);
    let count = fx
        .chain
        .view(fx.factory, factory_op::POOL_COUNT, &[])
        .expect("count");
    assert_eq!(count, 0u128.to_be_bytes().to_vec());
    let lookup = fx
        .chain
        .view(
            fx.factory,
            factory_op::GET_POOL,
            &factory_get_pool_args(&fx.za, &fx.zb),
        )
        .expect("lookup");
    assert!(lookup.is_empty(), "no pool registered");
}

#[test]
fn factory_009_spawn_failure_does_not_register() {
    let mut chain = DexChain::new();
    let alice = Holder::External(account("alice"));
    let token_a = chain.deploy(ContractKind::TestToken);
    let token_b = chain.deploy(ContractKind::TestToken);
    let (za, zb) = (
        AssetId::of_contract(&token_a),
        AssetId::of_contract(&token_b),
    );
    for (token, name) in [
        (token_a, "Zalkanes Test Asset A"),
        (token_b, "Zalkanes Test Asset B"),
    ] {
        chain
            .call(
                alice,
                token,
                token_op::INITIALIZE,
                &token_initialize_args(name),
                &[],
            )
            .unwrap();
        chain
            .call(
                alice,
                token,
                token_op::MINT_FOR_TEST,
                &token_mint_args(&alice, INIT_A * 10),
                &[],
            )
            .unwrap();
    }
    let factory = chain.deploy(ContractKind::SubfrostFactory);
    // Initialize the factory with a BAD template hash: spawn will fail.
    chain
        .call(
            alice,
            factory,
            factory_op::INITIALIZE,
            &zalkanes_dex_core::encode::factory_initialize_args(&[0xde; 32]),
            &[],
        )
        .expect("factory init");
    let root_before = chain.state_root();
    let err = chain
        .call(
            alice,
            factory,
            factory_op::CREATE_POOL,
            &factory_create_pool_args(&za, &zb),
            &[(za, INIT_A), (zb, INIT_B)],
        )
        .expect_err("spawn failure");
    assert_eq!(err, DexError::InvalidArguments);
    chain.rollback_to(chain.height() - 1);
    assert_eq!(chain.state_root(), root_before);
    let count = chain.view(factory, factory_op::POOL_COUNT, &[]).unwrap();
    assert_eq!(count, 0u128.to_be_bytes().to_vec());
}

#[test]
fn factory_010_hundred_pools_enumerate_identically() {
    // 100 sequential deterministic pairs on two independent chains must
    // enumerate identically (ids, order, count) and match state roots.
    fn build() -> (DexChain, ContractId, Vec<ContractId>) {
        let mut chain = DexChain::new();
        let alice = Holder::External(account("alice"));
        let factory = chain.deploy(ContractKind::SubfrostFactory);
        chain
            .call(
                alice,
                factory,
                factory_op::INITIALIZE,
                &zalkanes_dex_core::encode::factory_initialize_args(
                    &ContractKind::SubfrostPool.template_hash(),
                ),
                &[],
            )
            .expect("factory init");
        let mut pools = Vec::new();
        for i in 0..100u8 {
            // Deterministic distinct asset pairs (no token contract needed:
            // pools only need asset ids + custody, provided via faucet).
            let a = AssetId([i + 1; 32]);
            let b = AssetId([200u8.wrapping_add(i); 32]);
            chain.faucet(alice, a, 10_000_000);
            chain.faucet(alice, b, 10_000_000);
            let out = chain
                .call(
                    alice,
                    factory,
                    factory_op::CREATE_POOL,
                    &factory_create_pool_args(&a, &b),
                    &[(a, 2_000_000), (b, 2_000_000)],
                )
                .expect("create");
            pools.push(pool_id_from(out));
        }
        (chain, factory, pools)
    }
    let (chain1, factory1, pools1) = build();
    let (chain2, _factory2, pools2) = build();
    assert_eq!(pools1, pools2, "identical creation ids");
    assert_eq!(chain1.state_root(), chain2.state_root());
    let listed = decode_pool_list(
        &chain1
            .view(factory1, factory_op::GET_ALL_POOLS, &[])
            .expect("list"),
    )
    .expect("decode");
    assert_eq!(listed, pools1, "enumeration preserves creation order");
    // Pagination agrees with the full listing.
    let mut paged = Vec::new();
    for start in (0..100u32).step_by(7) {
        let mut args = start.to_be_bytes().to_vec();
        args.extend_from_slice(&7u32.to_be_bytes());
        let page = decode_pool_list(
            &chain1
                .view(factory1, factory_op::GET_ALL_POOLS, &args)
                .expect("page"),
        )
        .expect("decode page");
        paged.extend(page);
    }
    assert_eq!(paged, pools1);
    let count = chain1.view(factory1, factory_op::POOL_COUNT, &[]).unwrap();
    assert_eq!(count, 100u128.to_be_bytes().to_vec());
}

#[test]
fn factory_011_any_caller_can_create() {
    let mut fx = Fixture::new();
    // A brand-new account with funded balances can create a pool; no
    // owner gate exists.
    let mallory = Holder::External(account("mallory"));
    fx.chain.faucet(mallory, fx.za, 10_000_000);
    fx.chain.faucet(mallory, fx.zb, 10_000_000);
    let out = fx
        .chain
        .call(
            mallory,
            fx.factory,
            factory_op::CREATE_POOL,
            &factory_create_pool_args(&fx.za, &fx.zb),
            &[(fx.za, 2_000_000), (fx.zb, 2_000_000)],
        )
        .expect("permissionless create");
    let pool = pool_id_from(out);
    let lp = AssetId::of_contract(&pool);
    assert!(fx.chain.balance(&mallory, &lp) > 0);
}

#[test]
fn factory_012_no_hidden_admin_capability() {
    // Every opcode outside the public set fails with InvalidOpcode: no
    // owner ops, no fee setters, no collect, no pause.
    let (mut fx, _pool) = Fixture::with_pool();
    for opcode in [
        5u16,
        6,
        7,
        10,
        11,
        12,
        13,
        14,
        20,
        21,
        29,
        50,
        99,
        999,
        u16::MAX,
    ] {
        let err = fx
            .chain
            .call(fx.alice, fx.factory, opcode, &[], &[])
            .expect_err("no admin surface");
        assert_eq!(err, DexError::InvalidOpcode, "opcode {opcode}");
    }
}
