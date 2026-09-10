//! # zalkanes-protocol
//!
//! Canonical binary parser and serializer for Zalkanes v0 protocol messages.
//!
//! **Consensus-critical.** The parser is the source of truth for what
//! constitutes a valid Zalkanes message embedded in a Zcash transaction.

#![forbid(unsafe_code)]

use zalkanes_core::{
    consensus::{
        MAX_CODE_BYTES, MAX_INPUT_BYTES, MSG_CALL, MSG_DEPLOY, PROTOCOL_MAGIC, PROTOCOL_V0,
    },
    types::{CodeHash, ContractId},
};

/// A parsed Zalkanes protocol message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Message {
    Deploy(DeployMessage),
    Call(CallMessage),
}

/// A DEPLOY message parsed from an OP_RETURN payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeployMessage {
    /// SHA-256 of the WASM bytes — committed in effecting data.
    pub code_hash: CodeHash,
    /// Declared total byte length of the WASM module.
    pub code_length: u32,
    /// Number of P2SH carrier inputs carrying WASM chunks.
    pub chunk_count: u8,
    /// Output index of this OP_RETURN within the transaction.
    pub output_index: u16,
}

/// A CALL message parsed from an OP_RETURN payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallMessage {
    /// Target contract.
    pub contract_id: ContractId,
    /// Method selector.
    pub opcode: u16,
    /// Call input bytes.
    pub input: Vec<u8>,
}

#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("invalid magic bytes")]
    InvalidMagic,
    #[error("unknown protocol version {0}")]
    UnknownVersion(u8),
    #[error("unknown message type {0}")]
    UnknownMessageType(u8),
    #[error("truncated payload")]
    Truncated,
    #[error("trailing bytes after message")]
    TrailingBytes,
    #[error("code_length {0} exceeds MAX_CODE_BYTES {1}")]
    CodeLengthExceedsMax(u32, u32),
    #[error("input_length {0} exceeds MAX_INPUT_BYTES {1}")]
    InputLengthExceedsMax(u32, u32),
    #[error("declared input_length {declared} != actual bytes {actual}")]
    InputLengthMismatch { declared: u16, actual: usize },
}

/// Parse a Zalkanes protocol message from an OP_RETURN payload.
///
/// Returns `None` if the payload does not start with the ZALK magic (silent
/// skip). Returns `Err` if the magic matches but the message is malformed
/// (should be logged and counted as a protocol violation).
pub fn parse_op_return(payload: &[u8]) -> Result<Option<Message>, ParseError> {
    // Must have at least magic (4) + version (1) + type (1) = 6 bytes.
    if payload.len() < 6 {
        if payload.starts_with(&PROTOCOL_MAGIC[..payload.len().min(4)]) && payload.len() < 4 {
            return Err(ParseError::Truncated);
        }
        return Ok(None);
    }

    // Check magic — silent skip on mismatch.
    if payload[0..4] != PROTOCOL_MAGIC {
        return Ok(None);
    }

    let version = payload[4];
    if version != PROTOCOL_V0 {
        return Err(ParseError::UnknownVersion(version));
    }

    let msg_type = payload[5];
    let body = &payload[6..];

    let msg = match msg_type {
        MSG_DEPLOY => Message::Deploy(parse_deploy(body)?),
        MSG_CALL => Message::Call(parse_call(body)?),
        other => return Err(ParseError::UnknownMessageType(other)),
    };

    Ok(Some(msg))
}

fn parse_deploy(body: &[u8]) -> Result<DeployMessage, ParseError> {
    // code_hash (32) + code_length (4) + chunk_count (1) + output_index (2) = 39
    if body.len() < 39 {
        return Err(ParseError::Truncated);
    }
    if body.len() > 39 {
        return Err(ParseError::TrailingBytes);
    }

    let mut code_hash_bytes = [0u8; 32];
    code_hash_bytes.copy_from_slice(&body[0..32]);
    let code_hash = CodeHash(code_hash_bytes);

    let code_length = u32::from_be_bytes([body[32], body[33], body[34], body[35]]);
    if code_length == 0 || code_length > MAX_CODE_BYTES {
        return Err(ParseError::CodeLengthExceedsMax(
            code_length,
            MAX_CODE_BYTES,
        ));
    }

    let chunk_count = body[36];
    if chunk_count == 0 {
        return Err(ParseError::Truncated);
    }

    let output_index = u16::from_be_bytes([body[37], body[38]]);

    Ok(DeployMessage {
        code_hash,
        code_length,
        chunk_count,
        output_index,
    })
}

fn parse_call(body: &[u8]) -> Result<CallMessage, ParseError> {
    // contract_id (32) + opcode (2) + input_length (2) = 36 minimum
    if body.len() < 36 {
        return Err(ParseError::Truncated);
    }

    let mut id_bytes = [0u8; 32];
    id_bytes.copy_from_slice(&body[0..32]);
    let contract_id = ContractId(id_bytes);

    let opcode = u16::from_be_bytes([body[32], body[33]]);
    let input_length = u16::from_be_bytes([body[34], body[35]]);

    if input_length as u32 > MAX_INPUT_BYTES {
        return Err(ParseError::InputLengthExceedsMax(
            input_length as u32,
            MAX_INPUT_BYTES,
        ));
    }

    let input_data = &body[36..];
    if input_data.len() != input_length as usize {
        return Err(ParseError::InputLengthMismatch {
            declared: input_length,
            actual: input_data.len(),
        });
    }

    Ok(CallMessage {
        contract_id,
        opcode,
        input: input_data.to_vec(),
    })
}

/// Serialize a DEPLOY message into an OP_RETURN payload (max 80 bytes).
pub fn encode_deploy(msg: &DeployMessage) -> Vec<u8> {
    let mut out = Vec::with_capacity(6 + 39);
    out.extend_from_slice(&PROTOCOL_MAGIC);
    out.push(PROTOCOL_V0);
    out.push(MSG_DEPLOY);
    out.extend_from_slice(&msg.code_hash.0);
    out.extend_from_slice(&msg.code_length.to_be_bytes());
    out.push(msg.chunk_count);
    out.extend_from_slice(&msg.output_index.to_be_bytes());
    out
}

/// Serialize a CALL message into an OP_RETURN payload.
pub fn encode_call(msg: &CallMessage) -> Vec<u8> {
    let input_len = msg.input.len() as u16;
    let mut out = Vec::with_capacity(6 + 36 + msg.input.len());
    out.extend_from_slice(&PROTOCOL_MAGIC);
    out.push(PROTOCOL_V0);
    out.push(MSG_CALL);
    out.extend_from_slice(&msg.contract_id.0);
    out.extend_from_slice(&msg.opcode.to_be_bytes());
    out.extend_from_slice(&input_len.to_be_bytes());
    out.extend_from_slice(&msg.input);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_deploy_payload() -> Vec<u8> {
        let msg = DeployMessage {
            code_hash: CodeHash([0xABu8; 32]),
            code_length: 1024,
            chunk_count: 2,
            output_index: 1,
        };
        encode_deploy(&msg)
    }

    #[test]
    fn roundtrip_deploy() {
        let payload = make_deploy_payload();
        let parsed = parse_op_return(&payload).unwrap().unwrap();
        match parsed {
            Message::Deploy(d) => {
                assert_eq!(d.code_hash.0, [0xABu8; 32]);
                assert_eq!(d.code_length, 1024);
                assert_eq!(d.chunk_count, 2);
                assert_eq!(d.output_index, 1);
            }
            _ => panic!("expected deploy"),
        }
    }

    #[test]
    fn roundtrip_call() {
        let msg = CallMessage {
            contract_id: ContractId([0x11u8; 32]),
            opcode: 42,
            input: vec![1, 2, 3],
        };
        let payload = encode_call(&msg);
        let parsed = parse_op_return(&payload).unwrap().unwrap();
        match parsed {
            Message::Call(c) => {
                assert_eq!(c.contract_id.0, [0x11u8; 32]);
                assert_eq!(c.opcode, 42);
                assert_eq!(c.input, vec![1, 2, 3]);
            }
            _ => panic!("expected call"),
        }
    }

    #[test]
    fn wrong_magic_returns_none() {
        let payload = b"NOPE\x00\x01".to_vec();
        assert!(parse_op_return(&payload).unwrap().is_none());
    }

    #[test]
    fn unknown_version_returns_err() {
        let mut payload = make_deploy_payload();
        payload[4] = 0xFF;
        assert!(matches!(
            parse_op_return(&payload),
            Err(ParseError::UnknownVersion(0xFF))
        ));
    }

    #[test]
    fn trailing_bytes_rejected() {
        let mut payload = make_deploy_payload();
        payload.push(0x00);
        assert!(matches!(
            parse_op_return(&payload),
            Err(ParseError::TrailingBytes)
        ));
    }

    #[test]
    fn zero_code_length_rejected() {
        let msg = DeployMessage {
            code_hash: CodeHash([0u8; 32]),
            code_length: 0,
            chunk_count: 1,
            output_index: 0,
        };
        let payload = encode_deploy(&msg);
        assert!(matches!(
            parse_op_return(&payload),
            Err(ParseError::CodeLengthExceedsMax(0, _))
        ));
    }

    #[test]
    fn oversized_code_length_rejected() {
        let msg = DeployMessage {
            code_hash: CodeHash([0u8; 32]),
            code_length: MAX_CODE_BYTES + 1,
            chunk_count: 1,
            output_index: 0,
        };
        let payload = encode_deploy(&msg);
        assert!(matches!(
            parse_op_return(&payload),
            Err(ParseError::CodeLengthExceedsMax(_, _))
        ));
    }

    #[test]
    fn call_input_length_mismatch_rejected() {
        let msg = CallMessage {
            contract_id: ContractId([0u8; 32]),
            opcode: 0,
            input: vec![],
        };
        // Manually craft: declare 5 bytes but provide 0
        let mut payload = Vec::new();
        payload.extend_from_slice(&PROTOCOL_MAGIC);
        payload.push(PROTOCOL_V0);
        payload.push(MSG_CALL);
        payload.extend_from_slice(&msg.contract_id.0);
        payload.extend_from_slice(&0u16.to_be_bytes()); // opcode
        payload.extend_from_slice(&5u16.to_be_bytes()); // claim 5 bytes
                                                        // but add 0 input bytes
        assert!(matches!(
            parse_op_return(&payload),
            Err(ParseError::InputLengthMismatch {
                declared: 5,
                actual: 0
            })
        ));
    }
}
