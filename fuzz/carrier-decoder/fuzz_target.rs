#![no_main]
use libfuzzer_sys::fuzz_target;
use zalkanes_core::types::CodeHash;
use zalkanes_carrier::{reconstruct, Chunk};

fuzz_target!(|data: &[u8]| {
    // Parse first byte as chunk_count, next 4 bytes as code_length,
    // next 32 bytes as expected hash, rest as chunk data.
    if data.len() < 37 { return; }
    let chunk_count = data[0];
    let code_length = u32::from_be_bytes([data[1], data[2], data[3], data[4]]);
    let mut hash = [0u8; 32];
    hash.copy_from_slice(&data[5..37]);
    let code_hash = CodeHash(hash);
    let payload = &data[37..];

    // Produce one chunk for each byte in the remaining data (capped at chunk_count).
    let chunks: Vec<Chunk> = (0..chunk_count.min(255))
        .map(|i| Chunk {
            index: i,
            data: if payload.is_empty() { vec![] } else { vec![payload[i as usize % payload.len()]] },
        })
        .collect();

    // Must never panic.
    let _ = reconstruct(&chunks, chunk_count, code_length, &code_hash);
});
