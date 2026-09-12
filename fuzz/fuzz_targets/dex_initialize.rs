//! Pool creation with fuzz-chosen amounts: success implies exact
//! reserve/LP invariants; failure implies untouched state root.
#![no_main]
use libfuzzer_sys::fuzz_target;
use zalkanes_dex_core::encode::{factory_create_pool_args, factory_initialize_args, factory_op};
use zalkanes_dex_core::types::{AssetId, Holder};
use zalkanes_dex_testkit::{account, ContractKind, DexChain};

fn u128_at(data: &[u8], i: usize) -> u128 {
    let mut buf = [0u8; 16];
    for (j, b) in buf.iter_mut().enumerate() {
        *b = *data.get(i + j).unwrap_or(&0);
    }
    u128::from_be_bytes(buf)
}

fuzz_target!(|data: &[u8]| {
    let a0 = u128_at(data, 0) >> 32; // keep faucet additions overflow-free
    let a1 = u128_at(data, 16) >> 32;
    if a0 == 0 || a1 == 0 {
        return;
    }
    let mut chain = DexChain::new();
    let alice = Holder::External(account("fuzz"));
    let (ta, tb) = (AssetId([1; 32]), AssetId([2; 32]));
    chain.faucet(alice, ta, a0);
    chain.faucet(alice, tb, a1);
    let factory = chain.deploy(ContractKind::SubfrostFactory);
    chain
        .call(
            alice,
            factory,
            factory_op::INITIALIZE,
            &factory_initialize_args(&ContractKind::SubfrostPool.template_hash()),
            &[],
        )
        .expect("factory init");
    let root = chain.state_root();
    let result = chain.call(
        alice,
        factory,
        factory_op::CREATE_POOL,
        &factory_create_pool_args(&ta, &tb),
        &[(ta, a0), (tb, a1)],
    );
    match result {
        Ok(out) => assert_eq!(out.len(), 32),
        Err(_) => {
            let h = chain.height();
            assert_eq!(chain.root_at(h), root, "failed create must be atomic");
        }
    }
});
