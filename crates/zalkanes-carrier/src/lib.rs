//! # zalkanes-carrier
//!
//! P2SH carrier encoding and decoding for WASM contract deployment.
//!
//! See `docs/adr/0003-transaction-carrier.md` and `docs/protocol-v0.md §7`.

#![forbid(unsafe_code)]

use zalkanes_core::{consensus::MAX_CODE_BYTES, types::CodeHash};

/// A single chunk extracted from a carrier scriptSig.
#[derive(Debug, Clone)]
pub struct Chunk {
    pub index: u8,
    pub data: Vec<u8>,
}

/// Reconstruct WASM bytes from carrier chunks.
///
/// # Steps
/// 1. Check that exactly `chunk_count` distinct indexes 0..(chunk_count-1) are present.
/// 2. Sort by index.
/// 3. Concatenate data.
/// 4. Verify `len(concat) == expected_length`.
/// 5. Verify `SHA-256(concat) == expected_hash`.
pub fn reconstruct(
    chunks: &[Chunk],
    chunk_count: u8,
    expected_length: u32,
    expected_hash: &CodeHash,
) -> Result<Vec<u8>, CarrierError> {
    // Validate expected_length before allocation.
    if expected_length == 0 || expected_length > MAX_CODE_BYTES {
        return Err(CarrierError::InvalidLength(expected_length));
    }

    // Check we have the right number of chunks.
    if chunks.len() != chunk_count as usize {
        return Err(CarrierError::WrongChunkCount {
            expected: chunk_count,
            got: chunks.len() as u8,
        });
    }

    // Build index-checked sorted collection; detect duplicates.
    let mut indexed: Vec<Option<&[u8]>> = vec![None; chunk_count as usize];
    for chunk in chunks {
        if chunk.index >= chunk_count {
            return Err(CarrierError::ChunkOutOfRange {
                index: chunk.index,
                count: chunk_count,
            });
        }
        if indexed[chunk.index as usize].is_some() {
            return Err(CarrierError::DuplicateChunk(chunk.index));
        }
        indexed[chunk.index as usize] = Some(&chunk.data);
    }

    // All slots must be filled.
    let mut wasm = Vec::with_capacity(expected_length as usize);
    for (i, slot) in indexed.iter().enumerate() {
        match slot {
            Some(data) => wasm.extend_from_slice(data),
            None => return Err(CarrierError::MissingChunk(i as u8)),
        }
    }

    // Length check.
    if wasm.len() != expected_length as usize {
        return Err(CarrierError::LengthMismatch {
            expected: expected_length,
            got: wasm.len() as u32,
        });
    }

    // Hash check.
    let actual_hash = CodeHash::of(&wasm);
    if actual_hash.0 != expected_hash.0 {
        return Err(CarrierError::HashMismatch {
            expected: expected_hash.as_hex(),
            actual: actual_hash.as_hex(),
        });
    }

    Ok(wasm)
}

/// Split WASM bytes into carrier chunks of `chunk_size` bytes each.
///
/// Returns chunks with index 0, 1, 2, ...
pub fn split(wasm: &[u8], chunk_size: usize) -> Vec<Chunk> {
    assert!(chunk_size > 0, "chunk_size must be > 0");
    wasm.chunks(chunk_size)
        .enumerate()
        .map(|(i, data)| Chunk {
            index: i as u8,
            data: data.to_vec(),
        })
        .collect()
}

#[derive(Debug, thiserror::Error)]
pub enum CarrierError {
    #[error("invalid declared length {0}")]
    InvalidLength(u32),

    #[error("wrong chunk count: expected {expected}, got {got}")]
    WrongChunkCount { expected: u8, got: u8 },

    #[error("chunk index {index} out of range (count={count})")]
    ChunkOutOfRange { index: u8, count: u8 },

    #[error("duplicate chunk index {0}")]
    DuplicateChunk(u8),

    #[error("missing chunk {0}")]
    MissingChunk(u8),

    #[error("length mismatch: expected {expected}, got {got}")]
    LengthMismatch { expected: u32, got: u32 },

    #[error("hash mismatch: expected {expected}, got {actual}")]
    HashMismatch { expected: String, actual: String },
}

// ── Script parsing (OP_RETURN + carrier scriptSig) ───────────────────────────
//
// Zalkanes never executes Zcash script; it only *reads* the push-only data
// placed in OP_RETURN outputs and carrier scriptSigs. These helpers parse the
// canonical push encodings deterministically.

/// Extract the data payload of an OP_RETURN output.
///
/// Returns the full pushed payload if the script is `OP_RETURN <data>`,
/// otherwise `None`. Handles OP_0 and the standard 1/2/4-byte push opcodes.
pub fn extract_op_return_data(script: &[u8]) -> Option<Vec<u8>> {
    if script.first() != Some(&0x6a) {
        return None;
    }
    // Skip OP_RETURN opcode, then parse the single data push that follows.
    let rest = &script[1..];
    if rest.is_empty() {
        // Bare OP_RETURN — no data.
        return Some(Vec::new());
    }
    let (payload, consumed) = parse_push(rest)?;
    // OP_RETURN must contain exactly one data push (no trailing opcodes).
    if consumed != rest.len() {
        return None;
    }
    Some(payload)
}

/// Parse a single push opcode at the start of `script`.
///
/// Returns `(pushed_bytes, bytes_consumed)`.
fn parse_push(script: &[u8]) -> Option<(Vec<u8>, usize)> {
    let op = *script.first()?;
    match op {
        0x00 => Some((Vec::new(), 1)), // OP_0
        0x01..=0x4b => {
            let n = op as usize;
            if script.len() < 1 + n {
                return None;
            }
            Some((script[1..1 + n].to_vec(), 1 + n))
        }
        0x4c => {
            // OP_PUSHDATA1
            let n = *script.get(1)? as usize;
            if script.len() < 2 + n {
                return None;
            }
            Some((script[2..2 + n].to_vec(), 2 + n))
        }
        0x4d => {
            // OP_PUSHDATA2 (little-endian length)
            let n = u16::from_le_bytes([*script.get(1)?, *script.get(2)?]) as usize;
            if script.len() < 3 + n {
                return None;
            }
            Some((script[3..3 + n].to_vec(), 3 + n))
        }
        0x4e => {
            // OP_PUSHDATA4 (little-endian length)
            let n = u32::from_le_bytes([
                *script.get(1)?,
                *script.get(2)?,
                *script.get(3)?,
                *script.get(4)?,
            ]) as usize;
            if script.len() < 5 + n {
                return None;
            }
            Some((script[5..5 + n].to_vec(), 5 + n))
        }
        _ => None,
    }
}

/// Parse the pushes of a carrier scriptSig in order.
///
/// The DEPLOY carrier scriptSig is:
/// ```text
/// <chunk_index: u8> <chunk_data> <signature> <redeem_script>
/// ```
/// We return the raw pushed byte-strings in order so the caller can identify
/// `chunk_index` (first push, single byte) and `chunk_data` (second push).
pub fn parse_script_pushes(mut script: &[u8]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    while !script.is_empty() {
        match parse_push(script) {
            Some((payload, consumed)) => {
                out.push(payload);
                script = &script[consumed..];
            }
            None => break,
        }
    }
    out
}

/// Extract a carrier chunk from a scriptSig.
///
/// The scriptSig is:
/// ```text
/// PUSH(chunk_index) PUSH(chunk_data_part...)* PUSH(signature) PUSH(redeem_script)
/// ```
/// Returns `(chunk_index, chunk_data)` where `chunk_data` is the concatenation
/// of all pushes between the index and the signature/redeem script (chunk data
/// is split into ≤520-byte pushes to respect `MAX_SCRIPT_ELEMENT_SIZE`).
pub fn chunk_from_script_sig(script: &[u8]) -> Option<(u8, Vec<u8>)> {
    let pushes = parse_script_pushes(script);
    // Need at least: index, one chunk part, signature, redeem_script.
    if pushes.len() < 4 {
        return None;
    }
    let index = match pushes[0].as_slice() {
        [b] => *b,
        _ => return None,
    };
    // Chunk data = pushes[1 .. len-2] (excludes signature and redeem_script).
    let mut chunk_data = Vec::new();
    for part in &pushes[1..pushes.len() - 2] {
        chunk_data.extend_from_slice(part);
    }
    Some((index, chunk_data))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy_wasm(size: usize) -> Vec<u8> {
        (0..size).map(|i| (i % 251) as u8).collect()
    }

    #[test]
    fn roundtrip_single_chunk() {
        let wasm = dummy_wasm(512);
        let hash = CodeHash::of(&wasm);
        let chunks = split(&wasm, 1024);
        assert_eq!(chunks.len(), 1);
        let reconstructed = reconstruct(&chunks, 1, wasm.len() as u32, &hash).unwrap();
        assert_eq!(reconstructed, wasm);
    }

    #[test]
    fn roundtrip_multi_chunk() {
        let wasm = dummy_wasm(1000);
        let hash = CodeHash::of(&wasm);
        let chunks = split(&wasm, 300); // 4 chunks: 300+300+300+100
        assert_eq!(chunks.len(), 4);
        let reconstructed = reconstruct(&chunks, 4, wasm.len() as u32, &hash).unwrap();
        assert_eq!(reconstructed, wasm);
    }

    #[test]
    fn reordered_chunks_still_reconstruct() {
        let wasm = dummy_wasm(900);
        let hash = CodeHash::of(&wasm);
        let mut chunks = split(&wasm, 300);
        chunks.swap(0, 2); // reorder: now [2, 1, 0]
        let reconstructed = reconstruct(&chunks, 3, wasm.len() as u32, &hash).unwrap();
        assert_eq!(reconstructed, wasm);
    }

    #[test]
    fn missing_chunk_rejected() {
        let wasm = dummy_wasm(600);
        let hash = CodeHash::of(&wasm);
        let mut chunks = split(&wasm, 300);
        chunks.pop(); // remove last chunk
        assert!(matches!(
            reconstruct(&chunks, 2, wasm.len() as u32, &hash),
            Err(CarrierError::WrongChunkCount { .. })
        ));
    }

    #[test]
    fn duplicate_chunk_rejected() {
        let wasm = dummy_wasm(600);
        let hash = CodeHash::of(&wasm);
        let mut chunks = split(&wasm, 300);
        let dup = Chunk {
            index: 0,
            data: chunks[0].data.clone(),
        };
        chunks[1] = dup; // replace chunk 1 with a duplicate of chunk 0
        assert!(matches!(
            reconstruct(&chunks, 2, wasm.len() as u32, &hash),
            Err(CarrierError::DuplicateChunk(0))
        ));
    }

    #[test]
    fn mutated_byte_rejected() {
        let wasm = dummy_wasm(600);
        let hash = CodeHash::of(&wasm);
        let mut chunks = split(&wasm, 300);
        chunks[0].data[10] ^= 0xFF; // flip a bit
        assert!(matches!(
            reconstruct(&chunks, 2, wasm.len() as u32, &hash),
            Err(CarrierError::HashMismatch { .. })
        ));
    }

    #[test]
    fn zero_length_rejected() {
        let hash = CodeHash([0u8; 32]);
        let result = reconstruct(&[], 0, 0, &hash);
        assert!(result.is_err());
    }

    #[test]
    fn op_return_data_extraction() {
        // OP_RETURN <0x01 0x02 0x03>
        assert_eq!(
            extract_op_return_data(&[0x6a, 0x03, 0x01, 0x02, 0x03]),
            Some(vec![1, 2, 3])
        );
        // Bare OP_RETURN
        assert_eq!(extract_op_return_data(&[0x6a]), Some(vec![]));
        // Not OP_RETURN
        assert_eq!(extract_op_return_data(&[0x51, 0x03]), None);
        // OP_RETURN with PUSHDATA1
        let mut s = vec![0x6a, 0x4c, 0x80];
        s.extend(vec![0xAA; 0x80]);
        assert_eq!(extract_op_return_data(&s), Some(vec![0xAA; 0x80]));
    }

    #[test]
    fn chunk_from_script_sig_parses_pushes() {
        // <0x02> <0xAA 0xBB 0xCC> <sig...> <redeem...>
        let script = [
            0x01, 0x02, // push 1 byte: chunk_index = 2
            0x03, 0xAA, 0xBB, 0xCC, // push 3 bytes: chunk_data part 1
            0x01, 0x30, // (fake signature)
            0x01, 0x51, // (fake redeem)
        ];
        let (idx, data) = chunk_from_script_sig(&script).unwrap();
        assert_eq!(idx, 2);
        assert_eq!(data, vec![0xAA, 0xBB, 0xCC]);
    }

    #[test]
    fn chunk_from_script_sig_concatenates_multi_push_data() {
        // chunk_index, two chunk parts, signature, redeem.
        let script = [
            0x01, 0x00, // chunk_index = 0
            0x02, 0xAA, 0xBB, // chunk part 1
            0x02, 0xCC, 0xDD, // chunk part 2
            0x01, 0x30, // signature
            0x01, 0x51, // redeem
        ];
        let (idx, data) = chunk_from_script_sig(&script).unwrap();
        assert_eq!(idx, 0);
        assert_eq!(data, vec![0xAA, 0xBB, 0xCC, 0xDD]);
    }
}
