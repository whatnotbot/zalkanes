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
}
