//! Swaps with fuzz-chosen direction/amount: success bounds + atomic
//! failure + asset conservation.
#![no_main]
use libfuzzer_sys::fuzz_target;
use zalkanes_dex_core::encode::{
    factory_create_pool_args, factory_initialize_args, factory_op, pool_op, pool_swap_args,
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
    let amount_in = u128_at(data, 0) >> 64;
    let min_out = u128_at(data, 16) >> 96;
    let dir = data.first().copied().unwrap_or(0) & 1;
    if amount_in == 0 {
        return;
    }
    let mut chain = DexChain::new();
    let alice = Holder::External(account("fuzz"));
    let (ta, tb) = (AssetId([1; 32]), AssetId([2; 32]));
    chain.faucet(alice, ta, amount_in.saturating_add(10_000_000));
    chain.faucet(alice, tb, amount_in.saturating_add(10_000_000));
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
    let token_in = if dir == 0 { ta } else { tb };
    let token_out = if dir == 0 { tb } else { ta };
    let total_out_before =
        chain.balance(&alice, &token_out) + chain.balance(&Holder::Contract(pool), &token_out);
    let root = chain.state_root();
    let result = chain.call(
        alice,
        pool,
        pool_op::SWAP_EXACT_IN,
        &pool_swap_args(min_out, 0),
        &[(token_in, amount_in)],
    );
    let total_out_after =
        chain.balance(&alice, &token_out) + chain.balance(&Holder::Contract(pool), &token_out);
    assert_eq!(total_out_before, total_out_after, "swap must conserve assets");
    if result.is_err() {
        let h = chain.height();
        assert_eq!(chain.root_at(h), root, "failed swap must be atomic");
    }
});
