//! Script-level decoders that consensus feeds with raw chain bytes:
//! OP_RETURN payload extraction, push parsing, and carrier scriptSig
//! chunk interpretation. No panic, no unbounded allocation.

#![no_main]
use libfuzzer_sys::fuzz_target;
use zalkanes_carrier::{chunk_from_script_sig, extract_op_return_data, parse_script_pushes};

fuzz_target!(|data: &[u8]| {
    let _ = extract_op_return_data(data);
    let pushes = parse_script_pushes(data);
    // Parsed pushes can never exceed the input in total size (allocation
    // amplification guard).
    let total: usize = pushes.iter().map(|p| p.len()).sum();
    assert!(total <= data.len(), "push parsing must not amplify input");
    let _ = chunk_from_script_sig(data);
});
