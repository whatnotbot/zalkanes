//! Protocol manifest and manifest hash.
//!
//! The canonical machine-readable protocol manifest is `protocol/v0.toml`. The
//! manifest hash is SHA-256 over its exact bytes, so any consensus-affecting
//! change alters the hash. Nodes expose it via `zalkanes_getInfo` so operators
//! can detect protocol divergence.

#![forbid(unsafe_code)]

use sha2::{Digest, Sha256};

/// The canonical protocol v0 manifest, embedded at compile time.
pub const PROTOCOL_V0_MANIFEST: &str = include_str!("../../../protocol/v0.toml");

/// SHA-256 over the exact bytes of [`PROTOCOL_V0_MANIFEST`].
///
/// This is the `PROTOCOL_V0_MANIFEST_HASH`. It is deterministic because the
/// manifest is embedded at compile time; recomputing the hash is cheap (the
/// manifest is a few KiB).
pub fn protocol_manifest_hash() -> [u8; 32] {
    let digest = Sha256::digest(PROTOCOL_V0_MANIFEST.as_bytes());
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest);
    out
}

/// The manifest hash as lowercase hex.
pub fn protocol_manifest_hash_hex() -> String {
    hex::encode(protocol_manifest_hash())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_hashes_deterministically() {
        assert_eq!(protocol_manifest_hash(), protocol_manifest_hash());
        assert_eq!(protocol_manifest_hash_hex().len(), 64);
    }

    #[test]
    fn manifest_contains_key_constants() {
        assert!(PROTOCOL_V0_MANIFEST.contains("max_code_bytes = 262144"));
        assert!(PROTOCOL_V0_MANIFEST.contains("version = 5"));
        assert!(PROTOCOL_V0_MANIFEST.contains("mainnet_activation_height = \"None\""));
    }
}
