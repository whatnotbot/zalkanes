//! Protocol V1 wire format (ADR-0008).
//!
//! One new message type: `MSG_CALL_V1` (0x04) under version byte 0x01.
//! The OP_RETURN carries only a commitment (payload hash + length +
//! carrier count); the payload itself rides in P2SH carriers exactly like
//! v0 CALL_CARRIER calldata. v0 messages are untouched.

use zalkanes_core::consensus::{
    CALL_AUTH_PERSONALIZATION, MAX_CALL_V1_PAYLOAD_BYTES, MAX_V1_ATTACHED_ASSETS,
};
use zalkanes_core::types::{v1_external_account_id, ContractId, Network};

/// Parsed V1 CALL commitment (from OP_RETURN).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallV1Commitment {
    /// SHA-256 of the carrier payload bytes.
    pub payload_hash: [u8; 32],
    /// Exact payload byte length.
    pub payload_length: u32,
    /// Number of P2SH carrier inputs carrying payload chunks.
    pub carrier_count: u8,
}

/// Flags in the V1 payload.
pub const FLAG_HAS_AUTH: u8 = 0b0000_0001;
pub const FLAG_HAS_RECIPIENT: u8 = 0b0000_0010;

/// Decoded V1 CALL payload (from carriers).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallV1Payload {
    pub contract_id: ContractId,
    pub opcode: u16,
    /// Attached assets: (asset_id, amount), amounts nonzero, no duplicates.
    pub attached: Vec<([u8; 32], u128)>,
    /// Where contract outputs addressed to the caller land (33-byte holder).
    pub recipient: Option<[u8; 33]>,
    pub input: Vec<u8>,
    /// compressed secp256k1 pubkey + 64-byte compact ECDSA signature.
    pub auth: Option<([u8; 33], [u8; 64])>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum V1PayloadError {
    #[error("payload truncated")]
    Truncated,
    #[error("trailing bytes")]
    TrailingBytes,
    #[error("too many attached assets: {0}")]
    TooManyAssets(u8),
    #[error("duplicate attached asset")]
    DuplicateAsset,
    #[error("zero attached amount")]
    ZeroAmount,
    #[error("unknown flags {0:#04x}")]
    UnknownFlags(u8),
    #[error("attachments require auth")]
    AttachmentsWithoutAuth,
    #[error("payload exceeds maximum size")]
    TooLarge,
    #[error("invalid auth signature")]
    BadSignature,
}

/// Encode the OP_RETURN commitment: "ZALK" ‖ 0x01 ‖ 0x04 ‖ hash ‖ len ‖ count.
#[must_use]
pub fn encode_call_v1_commitment(c: &CallV1Commitment) -> Vec<u8> {
    use zalkanes_core::consensus::{MSG_CALL_V1, PROTOCOL_MAGIC, PROTOCOL_V1};
    let mut out = Vec::with_capacity(43);
    out.extend_from_slice(&PROTOCOL_MAGIC);
    out.push(PROTOCOL_V1);
    out.push(MSG_CALL_V1);
    out.extend_from_slice(&c.payload_hash);
    out.extend_from_slice(&c.payload_length.to_be_bytes());
    out.push(c.carrier_count);
    out
}

/// Encode the carrier payload. `auth = None` produces the exact bytes the
/// auth signature must sign (payload-without-auth, flags bit0 still set by
/// the final encoding when auth is appended — the SIGNED bytes always have
/// bit0 CLEAR so signing is stable).
#[must_use]
pub fn encode_call_v1_payload(p: &CallV1Payload) -> Vec<u8> {
    let mut flags = 0u8;
    if p.auth.is_some() {
        flags |= FLAG_HAS_AUTH;
    }
    if p.recipient.is_some() {
        flags |= FLAG_HAS_RECIPIENT;
    }
    let mut out = Vec::with_capacity(128 + p.input.len());
    out.extend_from_slice(&p.contract_id.0);
    out.extend_from_slice(&p.opcode.to_be_bytes());
    out.push(flags);
    out.push(p.attached.len() as u8);
    for (asset, amount) in &p.attached {
        out.extend_from_slice(asset);
        out.extend_from_slice(&amount.to_be_bytes());
    }
    if let Some(recipient) = &p.recipient {
        out.extend_from_slice(recipient);
    }
    out.extend_from_slice(&(p.input.len() as u32).to_be_bytes());
    out.extend_from_slice(&p.input);
    if let Some((pubkey, sig)) = &p.auth {
        out.extend_from_slice(pubkey);
        out.extend_from_slice(sig);
    }
    out
}

/// The exact bytes covered by the auth signature: the payload re-encoded
/// with `auth = None` and flags bit0 cleared.
#[must_use]
pub fn call_v1_signing_bytes(p: &CallV1Payload) -> Vec<u8> {
    let unsigned = CallV1Payload {
        auth: None,
        ..p.clone()
    };
    encode_call_v1_payload(&unsigned)
}

/// Decode + structurally validate a V1 payload.
pub fn decode_call_v1_payload(bytes: &[u8]) -> Result<CallV1Payload, V1PayloadError> {
    if bytes.len() as u32 > MAX_CALL_V1_PAYLOAD_BYTES {
        return Err(V1PayloadError::TooLarge);
    }
    let mut pos = 0usize;
    let take = |pos: &mut usize, n: usize| -> Result<&[u8], V1PayloadError> {
        let end = pos.checked_add(n).ok_or(V1PayloadError::Truncated)?;
        if end > bytes.len() {
            return Err(V1PayloadError::Truncated);
        }
        let out = &bytes[*pos..end];
        *pos = end;
        Ok(out)
    };

    let mut contract = [0u8; 32];
    contract.copy_from_slice(take(&mut pos, 32)?);
    let opcode_bytes = take(&mut pos, 2)?;
    let opcode = u16::from_be_bytes([opcode_bytes[0], opcode_bytes[1]]);
    let flags = take(&mut pos, 1)?[0];
    if flags & !(FLAG_HAS_AUTH | FLAG_HAS_RECIPIENT) != 0 {
        return Err(V1PayloadError::UnknownFlags(flags));
    }
    let attached_count = take(&mut pos, 1)?[0];
    if attached_count > MAX_V1_ATTACHED_ASSETS {
        return Err(V1PayloadError::TooManyAssets(attached_count));
    }
    let mut attached = Vec::with_capacity(attached_count as usize);
    for _ in 0..attached_count {
        let mut asset = [0u8; 32];
        asset.copy_from_slice(take(&mut pos, 32)?);
        let mut amount = [0u8; 16];
        amount.copy_from_slice(take(&mut pos, 16)?);
        let amount = u128::from_be_bytes(amount);
        if amount == 0 {
            return Err(V1PayloadError::ZeroAmount);
        }
        if attached.iter().any(|(a, _)| *a == asset) {
            return Err(V1PayloadError::DuplicateAsset);
        }
        attached.push((asset, amount));
    }
    let recipient = if flags & FLAG_HAS_RECIPIENT != 0 {
        let mut r = [0u8; 33];
        r.copy_from_slice(take(&mut pos, 33)?);
        if r[0] > 1 {
            return Err(V1PayloadError::UnknownFlags(r[0]));
        }
        Some(r)
    } else {
        None
    };
    let mut len_bytes = [0u8; 4];
    len_bytes.copy_from_slice(take(&mut pos, 4)?);
    let input_len = u32::from_be_bytes(len_bytes) as usize;
    let input = take(&mut pos, input_len)?.to_vec();
    let auth = if flags & FLAG_HAS_AUTH != 0 {
        let mut pubkey = [0u8; 33];
        pubkey.copy_from_slice(take(&mut pos, 33)?);
        let mut sig = [0u8; 64];
        sig.copy_from_slice(take(&mut pos, 64)?);
        Some((pubkey, sig))
    } else {
        None
    };
    if pos != bytes.len() {
        return Err(V1PayloadError::TrailingBytes);
    }
    if !attached.is_empty() && auth.is_none() {
        return Err(V1PayloadError::AttachmentsWithoutAuth);
    }
    Ok(CallV1Payload {
        contract_id: ContractId(contract),
        opcode,
        attached,
        recipient,
        input,
        auth,
    })
}

/// Auth sighash (ADR-0008 §4):
/// BLAKE2b-256("ZalkCallAuth1   ", network_id ‖ funding_prevout(36) ‖ signing_bytes).
#[must_use]
pub fn call_v1_auth_sighash(
    network: Network,
    funding_prevout: &[u8; 36],
    signing_bytes: &[u8],
) -> [u8; 32] {
    let mut input = Vec::with_capacity(1 + 36 + signing_bytes.len());
    input.push(network.id_byte());
    input.extend_from_slice(funding_prevout);
    input.extend_from_slice(signing_bytes);
    let hash = blake2b_simd::Params::new()
        .hash_length(32)
        .personal(CALL_AUTH_PERSONALIZATION)
        .hash(&input);
    let mut out = [0u8; 32];
    out.copy_from_slice(hash.as_bytes());
    out
}

/// Verify the payload's auth against the funding prevout. Returns the
/// authenticated external account id, or an error.
pub fn verify_call_v1_auth(
    network: Network,
    funding_prevout: &[u8; 36],
    payload: &CallV1Payload,
) -> Result<Option<[u8; 32]>, V1PayloadError> {
    let Some((pubkey_bytes, sig_bytes)) = &payload.auth else {
        return Ok(None);
    };
    let sighash = call_v1_auth_sighash(network, funding_prevout, &call_v1_signing_bytes(payload));
    let secp = secp256k1::Secp256k1::verification_only();
    let pubkey =
        secp256k1::PublicKey::from_slice(pubkey_bytes).map_err(|_| V1PayloadError::BadSignature)?;
    let message = secp256k1::Message::from_digest_slice(&sighash)
        .map_err(|_| V1PayloadError::BadSignature)?;
    let signature = secp256k1::ecdsa::Signature::from_compact(sig_bytes)
        .map_err(|_| V1PayloadError::BadSignature)?;
    secp.verify_ecdsa(&message, &signature, &pubkey)
        .map_err(|_| V1PayloadError::BadSignature)?;
    Ok(Some(v1_external_account_id(network, pubkey_bytes)))
}

/// Sign a payload (tooling/testkit side).
pub fn sign_call_v1(
    network: Network,
    funding_prevout: &[u8; 36],
    payload_without_auth: &CallV1Payload,
    secret_key: &secp256k1::SecretKey,
) -> ([u8; 33], [u8; 64]) {
    let secp = secp256k1::Secp256k1::new();
    let pubkey = secp256k1::PublicKey::from_secret_key(&secp, secret_key);
    let sighash = call_v1_auth_sighash(
        network,
        funding_prevout,
        &call_v1_signing_bytes(payload_without_auth),
    );
    let message = secp256k1::Message::from_digest_slice(&sighash).expect("32-byte digest");
    let signature = secp.sign_ecdsa(&message, secret_key);
    (pubkey.serialize(), signature.serialize_compact())
}
