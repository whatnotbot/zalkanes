//! Event decoding on arbitrary bytes; valid events must round-trip.
#![no_main]
use libfuzzer_sys::fuzz_target;
use zalkanes_dex_core::events::DexEvent;

fuzz_target!(|data: &[u8]| {
    if let Ok(event) = DexEvent::decode(data) {
        let encoded = event.encode();
        assert_eq!(DexEvent::decode(&encoded).unwrap(), event);
        assert_eq!(encoded, data, "canonical encoding must be unique");
    }
});
