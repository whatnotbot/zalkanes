//! Remove-liquidity with fuzz-chosen burn amount and minimums.
#![no_main]
use libfuzzer_sys::fuzz_target;
use zalkanes_dex_core::encode::{
    factory_create_pool_args, factory_initialize_args, factory_op, pool_op,
    pool_remove_liquidity_args,
};
use zalkanes_dex_core::types::{AssetId, ContractId, Holder};
use zalkanes_dex_testkit::{account, ContractKind, DexChain};

fn u128_at(data: &[u8], i: usize) -> u128 {
    let mut buf = [0u8; 16];
    for (j, b) in buf.iter_mut().enumerate() {
        *b = *data.get(i + j).unwrap_or(&0);
    }
    u128::from_be_bytes(buf)
}

fuzz_target!(|data: &[u8]| {
    let burn = u128_at(data, 0) % 6_000_000;
    let min0 = u128_at(data, 16) >> 100;
    let min1 = u128_at(data, 32) >> 100;
    if burn == 0 {
        return;
    }
    let mut chain = DexChain::new();
    let alice = Holder::External(account("fuzz"));
    let (ta, tb) = (AssetId([1; 32]), AssetId([2; 32]));
    chain.faucet(alice, ta, 10_000_000);
    chain.faucet(alice, tb, 10_000_000);
    let factory = chain.deploy(ContractKind::SubfrostFactory);
    chain
        .call(
            alice,
            factory,
            factory_op::INITIALIZE,
            &factory_initialize_args(&ContractKind::SubfrostPool.template_hash()),
            &[],
        )
        .unwrap();
    let out = chain
        .call(
            alice,
            factory,
            factory_op::CREATE_POOL,
            &factory_create_pool_args(&ta, &tb),
            &[(ta, 5_000_000), (tb, 5_000_000)],
        )
        .expect("pool");
    let mut pool = [0u8; 32];
    pool.copy_from_slice(&out);
    let pool = ContractId(pool);
    let lp = AssetId::of_contract(&pool);
    let held = chain.balance(&alice, &lp);
    let root = chain.state_root();
    let result = chain.call(
        alice,
        pool,
        pool_op::REMOVE_LIQUIDITY,
        &pool_remove_liquidity_args(min0, min1, 0),
        &[(lp, burn)],
    );
    match result {
        Ok(_) => assert!(burn <= held),
        Err(_) => {
            let h = chain.height();
            assert_eq!(chain.root_at(h), root, "failed remove must be atomic");
        }
    }
});
