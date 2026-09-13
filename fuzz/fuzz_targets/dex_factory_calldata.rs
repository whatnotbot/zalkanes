//! Factory calldata decoding + dispatch on arbitrary bytes: no panic,
//! deterministic results.
#![no_main]
use libfuzzer_sys::fuzz_target;
use zalkanes_dex_core::types::Holder;
use zalkanes_dex_testkit::{account, ContractKind, DexChain};

fuzz_target!(|data: &[u8]| {
    let _ = zalkanes_dex_core::encode::decode_factory_initialize_args(data);
    let _ = zalkanes_dex_core::encode::decode_factory_create_pool_args(data);
    if data.is_empty() {
        return;
    }
    let opcode = u16::from(data[0]);
    let mut chain = DexChain::new();
    let alice = Holder::External(account("fuzz"));
    let factory = chain.deploy(ContractKind::SubfrostFactory);
    let _ = chain.call(alice, factory, opcode, &data[1..], &[]);
});
