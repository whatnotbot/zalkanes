//! Carrier chunk reconstruction: no panic on arbitrary chunk sets, and any
//! successful reconstruction must actually match the demanded length + hash.

#![no_main]
use libfuzzer_sys::fuzz_target;
use sha2::{Digest, Sha256};
use zalkanes_carrier::{reconstruct, Chunk};
use zalkanes_core::types::CodeHash;

fuzz_target!(|data: &[u8]| {
    if data.len() < 40 {
        return;
    }
    // Structured header: chunk_count, declared length, expected hash, then
    // the remaining bytes are sliced into chunks with indices derived from
    // the data itself (duplicates / gaps / out-of-range all reachable).
    let chunk_count = data[0];
    let declared_len = u32::from_be_bytes([data[1], data[2], data[3], 0]);
    let mut hash = [0u8; 32];
    hash.copy_from_slice(&data[4..36]);
    let body = &data[36..];

    let mut chunks = Vec::new();
    let mut off = 0usize;
    while off + 2 <= body.len() && chunks.len() < 64 {
        let index = body[off];
        let take = (body[off + 1] as usize % 96) + 1;
        let end = std::cmp::min(off + 2 + take, body.len());
        chunks.push(Chunk {
            index,
            data: body[off + 2..end].to_vec(),
        });
        off = end;
    }

    if let Ok(bytes) = reconstruct(&chunks, chunk_count, declared_len, &CodeHash(hash)) {
        assert_eq!(bytes.len() as u32, declared_len, "length must be enforced");
        let actual: [u8; 32] = Sha256::digest(&bytes).into();
        assert_eq!(actual, hash, "hash must be enforced");
    }
});
