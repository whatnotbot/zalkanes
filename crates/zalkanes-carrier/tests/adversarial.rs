//! Adversarial tests for carrier chunk reconstruction.
//!
//! Reconstruction must reject (never panic) on any malformed chunk set, with
//! bounded allocation: `expected_length` is validated before allocation, and
//! the index-checked collection is sized by `chunk_count`.

use zalkanes_carrier::{reconstruct, split, CarrierError, Chunk};
use zalkanes_core::consensus::MAX_CODE_BYTES;
use zalkanes_core::types::CodeHash;

fn chunks_for(wasm: &[u8]) -> (Vec<Chunk>, u8, u32, CodeHash) {
    let chunks = split(wasm, 4);
    let n = chunks.len() as u8;
    let len = wasm.len() as u32;
    let hash = CodeHash::of(wasm);
    (chunks, n, len, hash)
}

#[test]
fn roundtrip() {
    let wasm = b"\x00asm\x01\x00\x00\x00 hello world";
    let (chunks, n, len, hash) = chunks_for(wasm);
    assert_eq!(reconstruct(&chunks, n, len, &hash).unwrap(), wasm);
}

#[test]
fn zero_chunks_declared() {
    let wasm = b"abc";
    let (_, _, len, hash) = chunks_for(wasm);
    // chunk_count=0 with non-empty expected length cannot reconstruct.
    assert!(reconstruct(&[], 0, len, &hash).is_err());
}

#[test]
fn missing_chunk() {
    let wasm = b"abcdefgh";
    let (mut chunks, n, len, hash) = chunks_for(wasm);
    chunks.pop(); // remove last chunk
    assert!(matches!(
        reconstruct(&chunks, n, len, &hash),
        Err(CarrierError::WrongChunkCount { .. })
    ));
}

#[test]
fn duplicate_chunk() {
    let wasm = b"abcdefgh";
    let (mut chunks, n, len, hash) = chunks_for(wasm);
    chunks.push(chunks[0].clone());
    assert!(matches!(
        reconstruct(&chunks, n, len, &hash),
        Err(CarrierError::WrongChunkCount { .. }) | Err(CarrierError::DuplicateChunk(_))
    ));
}

#[test]
fn out_of_range_chunk() {
    let wasm = b"abcdefgh";
    let (mut chunks, n, len, hash) = chunks_for(wasm);
    chunks[0] = Chunk {
        index: n,
        data: chunks[0].data.clone(),
    };
    assert!(matches!(
        reconstruct(&chunks, n, len, &hash),
        Err(CarrierError::ChunkOutOfRange { .. })
    ));
}

#[test]
fn reordered_chunks_are_sorted() {
    let wasm = b"abcdefgh";
    let (mut chunks, n, len, hash) = chunks_for(wasm);
    chunks.reverse(); // reverse order; reconstruction sorts by index
    assert_eq!(reconstruct(&chunks, n, len, &hash).unwrap(), wasm);
}

#[test]
fn wrong_length() {
    let wasm = b"abcdefgh";
    let (chunks, n, _, hash) = chunks_for(wasm);
    assert!(matches!(
        reconstruct(&chunks, n, 999, &hash),
        Err(CarrierError::LengthMismatch { .. })
    ));
}

#[test]
fn wrong_hash() {
    let wasm = b"abcdefgh";
    let (chunks, n, len, _) = chunks_for(wasm);
    let bad = CodeHash([0xEE; 32]);
    assert!(matches!(
        reconstruct(&chunks, n, len, &bad),
        Err(CarrierError::HashMismatch { .. })
    ));
}

#[test]
fn length_over_max_rejected_before_alloc() {
    let wasm = b"x";
    let (chunks, n, _, hash) = chunks_for(wasm);
    assert!(matches!(
        reconstruct(&chunks, n, MAX_CODE_BYTES + 1, &hash),
        Err(CarrierError::InvalidLength(_))
    ));
}
