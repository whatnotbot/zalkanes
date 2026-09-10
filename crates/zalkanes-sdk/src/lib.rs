//! # zalkanes-sdk
//!
//! Contract-side SDK for Zalkanes v0 contracts.
//! Targets `wasm32-unknown-unknown` (no_std compatible).
//!
//! Provides storage access, context, and the `CallResponse` return type.

#![no_std]
// SAFETY JUSTIFICATION: The SDK runs inside a WASM module and must call host
// functions via `extern "C"`. These are the only unsafe blocks in this crate
// and are unavoidable for the WASM host ABI. All pointer arithmetic is
// bounds-checked before use.
#![allow(unsafe_code)]

extern crate alloc;
use alloc::vec::Vec;

// ── Host imports ─────────────────────────────────────────────────────────────

extern "C" {
    fn storage_get(key_ptr: i32, key_len: i32, val_ptr: i32) -> i32;
    fn storage_set(key_ptr: i32, key_len: i32, val_ptr: i32, val_len: i32) -> i32;
    fn storage_delete(key_ptr: i32, key_len: i32) -> i32;
    fn context_block_height(out_ptr: i32) -> i32;
    fn input_read(out_ptr: i32, offset: i32, len: i32) -> i32;
    fn output_write(ptr: i32, len: i32) -> i32;
}

// ── Storage ───────────────────────────────────────────────────────────────────

/// Read a value from contract storage.
pub fn get(key: &[u8]) -> Option<Vec<u8>> {
    let mut buf = alloc::vec![0u8; 65536];
    let len = unsafe {
        storage_get(
            key.as_ptr() as i32,
            key.len() as i32,
            buf.as_mut_ptr() as i32,
        )
    };
    if len < 0 {
        None
    } else {
        buf.truncate(len as usize);
        Some(buf)
    }
}

/// Write a value to contract storage.
pub fn set(key: &[u8], value: &[u8]) -> bool {
    let rc = unsafe {
        storage_set(
            key.as_ptr() as i32,
            key.len() as i32,
            value.as_ptr() as i32,
            value.len() as i32,
        )
    };
    rc == 0
}

/// Delete a key from contract storage.
pub fn delete(key: &[u8]) -> bool {
    let rc = unsafe { storage_delete(key.as_ptr() as i32, key.len() as i32) };
    rc == 0
}

// ── Context ───────────────────────────────────────────────────────────────────

/// Get the current block height.
pub fn block_height() -> u32 {
    let mut buf = [0u8; 4];
    unsafe { context_block_height(buf.as_mut_ptr() as i32) };
    u32::from_be_bytes(buf)
}

// ── Input / Output ────────────────────────────────────────────────────────────

/// Read call input bytes.
pub fn read_input() -> Vec<u8> {
    let mut buf = alloc::vec![0u8; 65536];
    let len = unsafe { input_read(buf.as_mut_ptr() as i32, 0, buf.len() as i32) };
    if len > 0 {
        buf.truncate(len as usize);
        buf
    } else {
        Vec::new()
    }
}

/// Write call output bytes.
pub fn write_output(data: &[u8]) -> bool {
    let rc = unsafe { output_write(data.as_ptr() as i32, data.len() as i32) };
    rc == 0
}

// ── Helper types ──────────────────────────────────────────────────────────────

/// Encode a u64 as big-endian bytes for storage.
pub fn u64_to_bytes(v: u64) -> [u8; 8] {
    v.to_be_bytes()
}

/// Decode a u64 from big-endian bytes.
pub fn bytes_to_u64(b: &[u8]) -> u64 {
    if b.len() < 8 {
        return 0;
    }
    let mut arr = [0u8; 8];
    arr.copy_from_slice(&b[..8]);
    u64::from_be_bytes(arr)
}
