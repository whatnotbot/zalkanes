//! Pool calldata decoding + dispatch on arbitrary bytes: no panic.
#![no_main]
use libfuzzer_sys::fuzz_target;
use zalkanes_dex_core::types::Holder;
use zalkanes_dex_testkit::{account, ContractKind, DexChain};

fuzz_target!(|data: &[u8]| {
    let _ = zalkanes_dex_core::encode::decode_pool_initialize_args(data);
    let _ = zalkanes_dex_core::encode::decode_pool_add_liquidity_args(data);
    let _ = zalkanes_dex_core::encode::decode_pool_remove_liquidity_args(data);
    let _ = zalkanes_dex_core::encode::decode_pool_swap_args(data);
    let _ = zalkanes_dex_core::encode::decode_pool_quote_args(data);
    if data.is_empty() {
        return;
    }
    let opcode = u16::from(data[0]);
    let mut chain = DexChain::new();
    let caller = Holder::External(account("fuzz"));
    let pool = chain.deploy(ContractKind::SubfrostPool);
    let _ = chain.call(caller, pool, opcode, &data[1..], &[]);
});
