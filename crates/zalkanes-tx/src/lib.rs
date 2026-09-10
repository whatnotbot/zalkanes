//! # zalkanes-tx
//!
//! Transparent Zcash transaction construction for Zalkanes DEPLOY and CALL,
//! plus ZIP-317 fee calculation.
//!
//! Builds real transparent bundles via librustzcash's `TransparentBuilder`:
//! - DEPLOY: OP_RETURN (Zalkanes message) + P2SH carrier inputs (WASM chunks)
//! - CALL:   OP_RETURN (Zalkanes message) + a funding input
//!
//! Signing uses librustzcash's `apply_signatures` with a secp256k1 signing set;
//! the sighash is the caller's responsibility (ZIP-244 for v5+, computed from
//! the enclosing transaction context).

#![forbid(unsafe_code)]

use anyhow::Result;
use zalkanes_core::consensus::MAX_CODE_BYTES;
use zcash_protocol::value::Zatoshis;
use zcash_script::script::Code;
use zcash_transparent::{
    address::TransparentAddress,
    builder::{TransparentBuilder, TransparentSigningSet},
    bundle::{OutPoint, TxOut},
};

/// Estimated transaction size in bytes for fee calculation.
pub struct TxSizeEstimate {
    pub version_bytes: usize,
    pub inputs: usize,
    pub outputs: usize,
    pub input_bytes: usize,
    pub output_bytes: usize,
}

impl TxSizeEstimate {
    /// Estimate for a deployment transaction with N carrier inputs.
    pub fn for_deploy(carrier_inputs: usize) -> Self {
        Self {
            version_bytes: 4,
            inputs: carrier_inputs,
            outputs: 2, // OP_RETURN + change
            input_bytes: carrier_inputs * 200,
            output_bytes: 114,
        }
    }

    pub fn total_bytes(&self) -> usize {
        self.version_bytes + self.input_bytes + self.output_bytes + 10
    }
}

/// Calculate ZIP-317 fee in zatoshi (5000 zatoshi per logical action, min 2).
pub fn zip317_fee(inputs: usize, outputs: usize) -> u64 {
    const MARGINAL_FEE: u64 = 5_000;
    const GRACE_ACTIONS: u64 = 2;
    let logical_actions = inputs.max(outputs) as u64;
    let actions = logical_actions.max(GRACE_ACTIONS);
    MARGINAL_FEE * actions
}

/// Fee for a deployment transaction with N carrier inputs.
pub fn deployment_fee(carrier_inputs: usize) -> u64 {
    let est = TxSizeEstimate::for_deploy(carrier_inputs);
    zip317_fee(est.inputs, est.outputs)
}

/// Fee for a call transaction (1 input + OP_RETURN + change).
pub fn call_fee() -> u64 {
    zip317_fee(1, 2)
}

/// Maximum carrier chunk payload size for a single P2SH scriptSig push.
///
/// Zcash standard script pushes: OP_PUSHDATA4 allows up to u32::MAX, but relay
/// policy and the 512 KiB MAX_CODE_BYTES bound practical sizes. We use 4 KiB
/// per chunk, giving ≤ 128 inputs for a max-size contract.
pub const CHUNK_SIZE: usize = 4096;

/// Number of carrier inputs required for a given WASM byte length.
pub fn carrier_input_count(wasm_len: usize) -> Result<u8> {
    if wasm_len == 0 || wasm_len as u32 > MAX_CODE_BYTES {
        anyhow::bail!("invalid WASM length {wasm_len}");
    }
    let n = wasm_len.div_ceil(CHUNK_SIZE);
    if n > 255 {
        anyhow::bail!("WASM requires {n} chunks, exceeding the 255-input carrier limit");
    }
    Ok(n as u8)
}

/// Build a transparent bundle for a DEPLOY transaction.
///
/// - `op_return`: the encoded Zalkanes DEPLOY message (already `ZALK`-prefixed).
/// - `carrier_inputs`: (redeem_script, utxo, prevout_coin, chunk_script_sig) —
///   one per WASM chunk. The chunk scriptSig is the `<chunk_index> <chunk_data>
///   <signature> <redeem_script>` push layout per ADR 0003.
/// - `change`: change recipient + amount.
pub fn build_deploy_bundle(
    op_return: &[u8],
    change: Option<(&TransparentAddress, u64)>,
) -> Result<TransparentBuilder> {
    let mut builder = TransparentBuilder::empty();
    builder.add_null_data_output(op_return)?;
    if let Some((addr, value)) = change {
        builder.add_output(addr, Zatoshis::from_u64(value)?)?;
    }
    Ok(builder)
}

/// Add a carrier P2SH input to a builder.
///
/// The redeem script is `<deployer_pubkey> OP_CHECKSIG` (per ADR 0003); the
/// caller supplies it as raw script bytes together with the UTXO it spends.
pub fn add_carrier_input(
    builder: &mut TransparentBuilder,
    redeem_script: &[u8],
    prevout_txid: [u8; 32],
    prevout_index: u32,
    prevout_value: u64,
    prevout_script_pubkey: &[u8],
) -> Result<()> {
    let redeem = zcash_script::script::FromChain::parse(&Code(redeem_script.to_vec()))
        .map_err(|e| anyhow::anyhow!("invalid redeem script: {e}"))?;
    let utxo = OutPoint::new(prevout_txid, prevout_index);
    let coin = TxOut::new(
        Zatoshis::from_u64(prevout_value)?,
        zcash_transparent::address::Script(Code(prevout_script_pubkey.to_vec())),
    );
    builder.add_p2sh_input(redeem, utxo, coin)?;
    Ok(())
}

/// A transparent signing key (secp256k1 secret key) plus its pubkey.
#[derive(Clone)]
pub struct SigningKey {
    pub secret: secp256k1::SecretKey,
    pub public: secp256k1::PublicKey,
}

impl SigningKey {
    pub fn from_secret_bytes(bytes: [u8; 32]) -> Result<Self> {
        let secp = secp256k1::Secp256k1::new();
        let secret = secp256k1::SecretKey::from_slice(&bytes)
            .map_err(|e| anyhow::anyhow!("invalid secret key: {e}"))?;
        let public = secp256k1::PublicKey::from_secret_key(&secp, &secret);
        Ok(Self { secret, public })
    }

    /// A deterministic test key (NOT for production).
    pub fn test_key() -> Self {
        Self::from_secret_bytes([0x11; 32]).expect("valid test key")
    }
}

/// Build a signing set from keys (for `apply_signatures`).
pub fn signing_set(keys: &[SigningKey]) -> TransparentSigningSet {
    let mut set = TransparentSigningSet::new();
    for k in keys {
        set.add_key(k.secret);
    }
    set
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zip317_minimum_fee() {
        assert_eq!(zip317_fee(1, 1), 10_000);
    }

    #[test]
    fn zip317_scales_with_inputs() {
        assert!(zip317_fee(10, 2) > zip317_fee(2, 2));
    }

    #[test]
    fn carrier_input_count_matches_chunking() {
        assert_eq!(carrier_input_count(1).unwrap(), 1);
        assert_eq!(carrier_input_count(CHUNK_SIZE).unwrap(), 1);
        assert_eq!(carrier_input_count(CHUNK_SIZE + 1).unwrap(), 2);
    }

    #[test]
    fn carrier_input_count_rejects_empty_and_oversized() {
        assert!(carrier_input_count(0).is_err());
        assert!(carrier_input_count(MAX_CODE_BYTES as usize + 1).is_err());
    }

    #[test]
    fn deploy_bundle_roundtrips_build() {
        let op_return = b"ZALK\x00\x01".to_vec();
        let b = build_deploy_bundle(&op_return, None).unwrap();
        assert!(b.build().is_some());
    }

    #[test]
    fn signing_key_from_bytes() {
        let k = SigningKey::test_key();
        assert_eq!(
            k.public,
            secp256k1::PublicKey::from_secret_key(&secp256k1::Secp256k1::new(), &k.secret)
        );
    }
}
