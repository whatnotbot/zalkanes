//! ZALK OP_RETURN frame parser: no panic, no unbounded allocation, and
//! CANONICAL acceptance — any accepted message must re-encode to exactly the
//! input bytes (no nondeterministic or non-canonical acceptance).

#![no_main]
use libfuzzer_sys::fuzz_target;
use zalkanes_protocol::{encode_call, encode_call_carrier, encode_deploy, parse_op_return, Message};

fuzz_target!(|data: &[u8]| {
    match parse_op_return(data) {
        Ok(Some(msg)) => {
            let reencoded = match &msg {
                Message::Deploy(d) => encode_deploy(d),
                Message::Call(c) => encode_call(c),
                Message::CallCarrier(c) => encode_call_carrier(c),
            };
            assert_eq!(
                reencoded, data,
                "accepted message must re-encode canonically"
            );
        }
        Ok(None) => {}
        Err(_) => {}
    }
});
