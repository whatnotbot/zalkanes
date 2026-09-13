//! Pair canonicalization + Holder decoding must never panic and must
//! stay involutive on arbitrary input.
#![no_main]
use libfuzzer_sys::fuzz_target;
use zalkanes_dex_core::types::{AssetId, Holder, Pair};

fuzz_target!(|data: &[u8]| {
    let _ = Holder::from_bytes(data);
    if data.len() >= 64 {
        let mut a = [0u8; 32];
        let mut b = [0u8; 32];
        a.copy_from_slice(&data[..32]);
        b.copy_from_slice(&data[32..64]);
        let (a, b) = (AssetId(a), AssetId(b));
        match (Pair::canonical(a, b), Pair::canonical(b, a)) {
            (Ok(x), Ok(y)) => {
                assert_eq!(x, y);
                assert_eq!(x.key(), y.key());
                assert!(x.token0 < x.token1);
            }
            (Err(e1), Err(e2)) => assert_eq!(e1, e2),
            _ => panic!("canonical() asymmetric"),
        }
    }
});
