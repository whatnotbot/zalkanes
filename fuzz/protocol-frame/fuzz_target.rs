#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // Must never panic for any input.
    let _ = zalkanes_protocol::parse_op_return(data);
});
