//! Adversarial parser tests for the Zalkanes protocol decoder.
//!
//! No malformed payload may panic or return anything other than `Ok(None)`
//! (silent skip) or `Err` (protocol violation, no state mutation). The decoder
//! is pure: it never allocates unboundedly and never mutates state.

use zalkanes_core::consensus::{
    MAX_CALL_INLINE_BYTES, MAX_CODE_BYTES, MSG_CALL, PROTOCOL_MAGIC, PROTOCOL_V0,
};
use zalkanes_core::types::{CodeHash, ContractId};
use zalkanes_protocol::{
    encode_call, encode_call_carrier, encode_deploy, parse_op_return, CallCarrierMessage,
    CallMessage, DeployMessage,
};

const M: [u8; 4] = PROTOCOL_MAGIC;

fn deploy() -> DeployMessage {
    DeployMessage {
        code_hash: CodeHash([0xAB; 32]),
        code_length: 1024,
        chunk_count: 2,
        output_index: 0,
    }
}

fn call() -> CallMessage {
    CallMessage {
        contract_id: ContractId([0x11; 32]),
        opcode: 1,
        input: vec![1, 2, 3],
    }
}

#[test]
fn empty_payload() {
    assert_eq!(parse_op_return(b"").unwrap(), None);
}

#[test]
fn truncated_header() {
    // magic-only, version-only, etc.
    for len in 0..6 {
        let p = vec![0x5A; len];
        let _ = parse_op_return(&p); // must not panic
    }
}

#[test]
fn wrong_magic() {
    assert_eq!(parse_op_return(b"NOPE").unwrap(), None);
}

#[test]
fn unknown_version() {
    let mut p = encode_deploy(&deploy());
    p[4] = 0xFF;
    assert!(parse_op_return(&p).is_err());
}

#[test]
fn unknown_message_type() {
    let mut p = encode_deploy(&deploy());
    p[5] = 0x7F;
    assert!(parse_op_return(&p).is_err());
}

#[test]
fn trailing_bytes_after_deploy() {
    let mut p = encode_deploy(&deploy());
    p.push(0x00);
    assert!(parse_op_return(&p).is_err());
}

#[test]
fn trailing_bytes_after_call() {
    let mut p = encode_call(&call());
    p.push(0x00);
    assert!(parse_op_return(&p).is_err());
}

#[test]
fn code_length_zero() {
    let d = DeployMessage {
        code_length: 0,
        ..deploy()
    };
    assert!(parse_op_return(&encode_deploy(&d)).is_err());
}

#[test]
fn code_length_over_max() {
    let d = DeployMessage {
        code_length: MAX_CODE_BYTES + 1,
        ..deploy()
    };
    assert!(parse_op_return(&encode_deploy(&d)).is_err());
}

#[test]
fn zero_chunks_deploy() {
    let d = DeployMessage {
        chunk_count: 0,
        ..deploy()
    };
    assert!(parse_op_return(&encode_deploy(&d)).is_err());
}

#[test]
fn inline_input_over_limit() {
    // Declare > MAX_CALL_INLINE_BYTES input; must be rejected.
    let big = vec![0u8; (MAX_CALL_INLINE_BYTES + 1) as usize];
    let mut p = Vec::new();
    p.extend_from_slice(&M);
    p.push(PROTOCOL_V0);
    p.push(MSG_CALL);
    p.extend_from_slice(&[0x11; 32]);
    p.extend_from_slice(&1u16.to_be_bytes());
    p.extend_from_slice(&(big.len() as u16).to_be_bytes());
    p.extend_from_slice(&big);
    assert!(parse_op_return(&p).is_err());
}

#[test]
fn inline_input_length_mismatch() {
    let mut p = Vec::new();
    p.extend_from_slice(&M);
    p.push(PROTOCOL_V0);
    p.push(MSG_CALL);
    p.extend_from_slice(&[0x11; 32]);
    p.extend_from_slice(&1u16.to_be_bytes());
    p.extend_from_slice(&5u16.to_be_bytes()); // claim 5
                                              // but provide 0 input bytes
    assert!(parse_op_return(&p).is_err());
}

#[test]
fn call_carrier_roundtrip() {
    let cc = CallCarrierMessage {
        contract_id: ContractId([0x22; 32]),
        opcode: 3,
        input_hash: [0xAB; 32],
        input_length: 5000,
        carrier_count: 4,
    };
    let parsed = parse_op_return(&encode_call_carrier(&cc)).unwrap().unwrap();
    match parsed {
        zalkanes_protocol::Message::CallCarrier(c) => {
            assert_eq!(c.contract_id.0, [0x22; 32]);
            assert_eq!(c.opcode, 3);
            assert_eq!(c.input_hash, [0xAB; 32]);
            assert_eq!(c.input_length, 5000);
            assert_eq!(c.carrier_count, 4);
        }
        _ => panic!("expected call_carrier"),
    }
}

#[test]
fn call_carrier_zero_length() {
    let mut cc = CallCarrierMessage {
        contract_id: ContractId([0x22; 32]),
        opcode: 3,
        input_hash: [0xAB; 32],
        input_length: 0,
        carrier_count: 1,
    };
    assert!(parse_op_return(&encode_call_carrier(&cc)).is_err());
    cc.input_length = 1;
    cc.carrier_count = 0;
    assert!(parse_op_return(&encode_call_carrier(&cc)).is_err());
}

#[test]
fn multiple_messages_in_one_output_rejected_as_trailing() {
    // Two valid DEPLOY payloads concatenated: trailing bytes must be rejected.
    let mut p = encode_deploy(&deploy());
    let second = encode_deploy(&deploy());
    p.extend_from_slice(&second);
    assert!(parse_op_return(&p).is_err());
}
