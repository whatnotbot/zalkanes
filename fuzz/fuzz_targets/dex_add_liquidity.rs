//! Add-liquidity over a live pool with fuzz-chosen desired amounts.
#![no_main]
use libfuzzer_sys::fuzz_target;
use zalkanes_dex_core::encode::{
    factory_create_pool_args, factory_initialize_args, factory_op, pool_add_liquidity_args,
    pool_op,
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
    let d0 = (u128_at(data, 0) >> 64).max(0);
    let d1 = (u128_at(data, 16) >> 64).max(0);
    let min_lp = u128_at(data, 32) >> 96;
    if d0 == 0 || d1 == 0 {
        return;
    }
    let mut chain = DexChain::new();
    let alice = Holder::External(account("fuzz"));
    let (ta, tb) = (AssetId([1; 32]), AssetId([2; 32]));
    chain.faucet(alice, ta, d0.saturating_add(10_000_000));
    chain.faucet(alice, tb, d1.saturating_add(10_000_000));
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
    let root = chain.state_root();
    let result = chain.call(
        alice,
        pool,
        pool_op::ADD_LIQUIDITY,
        &pool_add_liquidity_args(min_lp, 0),
        &[(ta, d0), (tb, d1)],
    );
    if result.is_err() {
        let h = chain.height();
        assert_eq!(chain.root_at(h), root, "failed add must be atomic");
    }
});
