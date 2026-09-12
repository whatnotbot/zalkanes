//! View-result decoders on arbitrary bytes: no panic, strict length.
#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = zalkanes_dex_core::encode::decode_reserves(data);
    let _ = zalkanes_dex_core::encode::decode_pool_details(data);
    let _ = zalkanes_dex_core::encode::decode_swap_quote(data);
    let _ = zalkanes_dex_core::encode::decode_pool_list(data);
    let _ = zalkanes_dex_core::encode::decode_token_initialize_args(data);
    let _ = zalkanes_dex_core::encode::decode_token_mint_args(data);
});
