//! # zalkanes-tx
//!
//! Transparent Zcash transaction construction, signing, and serialization for
//! Zalkanes PREPARE / DEPLOY / CALL.
//!
//! Uses librustzcash's canonical types and sighash implementation:
//! - transaction version V4 (Canopy) on regtest
//! - `zcash_primitives::transaction::sighash_v4::v4_signature_hash`
//! - manual scriptSig construction to preserve carrier chunk pushes (the
//!   high-level `TransparentBuilder::apply_signatures` cannot represent the
//!   non-standard carrier redeem script).

#![forbid(unsafe_code)]

use anyhow::{bail, Result};
use zalkanes_core::consensus::MAX_CODE_BYTES;
use zcash_primitives::transaction::{
    sighash::{signature_hash, SignableInput as PrimitivesSignableInput},
    txid::TxIdDigester,
    Authorization, Authorized, Transaction, TransactionData, TxVersion,
};
use zcash_protocol::{
    consensus::{BlockHeight, BranchId},
    value::Zatoshis,
};
use zcash_script::script::Code;
use zcash_transparent::{
    address::{Script, TransparentAddress},
    builder::TransparentBuilder,
    bundle::{Authorized as TAuthorized, Bundle, OutPoint, TxIn, TxOut},
    sighash::{SighashType, SignableInput as TransparentSignableInput, SIGHASH_ALL},
};

/// ZIP-317 conventional fee (5000 zatoshi per logical action, min 2).
///
/// Logical actions are computed from serialized sizes, per ZIP-317:
/// `ceil(total_input_size / 150)` and `ceil(total_output_size / 34)`.
pub fn zip317_fee(inputs: usize, outputs: usize) -> u64 {
    const MARGINAL_FEE: u64 = 5_000;
    const GRACE_ACTIONS: u64 = 2;
    let logical_actions = inputs.max(outputs) as u64;
    let actions = logical_actions.max(GRACE_ACTIONS);
    MARGINAL_FEE * actions
}

/// ZIP-317 conventional fee from serialized input/output byte sizes.
///
/// This is the authoritative fee for carrier transactions, whose scriptSigs
/// are large and therefore consume many logical actions.
pub fn zip317_fee_from_sizes(total_input_bytes: usize, total_output_bytes: usize) -> u64 {
    const MARGINAL_FEE: u64 = 5_000;
    const GRACE_ACTIONS: u64 = 2;
    const P2PKH_STANDARD_INPUT_SIZE: usize = 150;
    const P2PKH_STANDARD_OUTPUT_SIZE: usize = 34;

    let in_actions = total_input_bytes.div_ceil(P2PKH_STANDARD_INPUT_SIZE);
    let out_actions = total_output_bytes.div_ceil(P2PKH_STANDARD_OUTPUT_SIZE);
    let logical = in_actions.max(out_actions) as u64;
    let actions = logical.max(GRACE_ACTIONS);
    MARGINAL_FEE * actions
}

// ── Carrier constants (derived from Zebra standardness) ──────────────────────

/// Maximum single-push data size in Zcash script (`MAX_SCRIPT_ELEMENT_SIZE`).
pub const MAX_PUSH_SIZE: usize = 520;

/// Zebra's `MAX_STANDARD_SCRIPTSIG_SIZE` (zcashd policy constant).
pub const MAX_STANDARD_SCRIPTSIG_SIZE: usize = 1650;

/// The carrier redeem script is non-standard as a solver template but
/// consensus-valid and contains 1 sigop:
///
/// ```text
/// <deployer_pubkey(33 bytes)> OP_CHECKSIG OP_NOP
/// ```
///
/// Zebra's `are_inputs_standard` accepts a P2SH spend whose redeemed script is
/// non-standard provided its sigop count ≤ 15, and explicitly permits "extra
/// data left on the stack after execution". The scriptSig is therefore:
///
/// ```text
/// PUSH(chunk_index) PUSH(chunk_data...)* PUSH(signature) PUSH(redeem_script)
/// ```
///
/// where `chunk_data` is split into ≤520-byte pushes (Zcash's per-push
/// `MAX_SCRIPT_ELEMENT_SIZE`). The whole scriptSig is push-only and ≤1650 bytes.
pub fn redeem_script(pubkey: &[u8; 33]) -> Vec<u8> {
    let mut s = Vec::with_capacity(36);
    s.push(0x21); // push 33 bytes
    s.extend_from_slice(pubkey);
    s.push(0xac); // OP_CHECKSIG
    s.push(0x61); // OP_NOP
    s
}

/// Serialized length of the redeem script (fixed at 36 bytes).
pub const REDEEM_SCRIPT_LEN: usize = 36;

/// Maximum DER-encoded ECDSA signature length (72 bytes) + sighash byte (1).
pub const MAX_SIGNATURE_LEN: usize = 73;

/// Push-opcode overhead for `data.len()` bytes.
fn push_overhead(len: usize) -> usize {
    match len {
        0..=75 => 1,
        76..=255 => 2,
        _ => 3, // OP_PUSHDATA2
    }
}

/// Maximum carrier chunk payload that fits within `MAX_STANDARD_SCRIPTSIG_SIZE`.
///
/// The chunk data is split into `MAX_PUSH_SIZE` (520) byte pushes. This returns
/// the total payload budget accounting for all push opcode overhead.
pub fn max_standard_carrier_payload(signature_len: usize, redeem_script_len: usize) -> usize {
    let chunk_index = 2; // 0x01 <index>
    let sig_push = 1 + signature_len;
    let redeem_push = push_overhead(redeem_script_len) + redeem_script_len;
    let fixed = chunk_index + sig_push + redeem_push;
    let remaining = MAX_STANDARD_SCRIPTSIG_SIZE - fixed;

    // Each 520-byte chunk push costs 3 bytes (OP_PUSHDATA2) overhead.
    // n full pushes + possibly one partial.
    let pushes = remaining / (MAX_PUSH_SIZE + 3);
    let leftover = remaining % (MAX_PUSH_SIZE + 3);
    pushes * MAX_PUSH_SIZE + leftover.saturating_sub(3)
}

/// The frozen protocol chunk payload size.
///
/// Derived empirically against the live Zebra regtest:
///
/// - per-push limit 520 bytes (Zcash `MAX_SCRIPT_ELEMENT_SIZE`)
/// - scriptSig limit 1650 bytes
/// - `max_standard_carrier_payload(73, 36)` = 1466 bytes
///
/// We freeze a conservative **1400 bytes** (≈1.37 KiB) to leave headroom for
/// DER signature length variance.
pub const CHUNK_PAYLOAD_SIZE: usize = 1400;

/// Maximum v0 contract size implied by 255 chunks of `CHUNK_PAYLOAD_SIZE`.
pub const MAX_CARRIER_CONTRACT_SIZE: usize = 255 * CHUNK_PAYLOAD_SIZE;

/// Number of carrier inputs required for a WASM byte length.
pub fn carrier_input_count(wasm_len: usize) -> Result<u8> {
    if wasm_len == 0 || wasm_len as u32 > MAX_CODE_BYTES {
        bail!("invalid WASM length {wasm_len}");
    }
    let n = wasm_len.div_ceil(CHUNK_PAYLOAD_SIZE);
    if n > 255 {
        bail!("WASM requires {n} chunks, exceeding the 255-input carrier limit");
    }
    Ok(n as u8)
}

// ── Script helpers ───────────────────────────────────────────────────────────

/// Encode a standard data push (push-only script fragment).
fn push_data(data: &[u8]) -> Vec<u8> {
    let n = data.len();
    if n < 0x4c {
        let mut v = Vec::with_capacity(1 + n);
        v.push(n as u8);
        v.extend_from_slice(data);
        v
    } else if n <= 0xff {
        let mut v = Vec::with_capacity(2 + n);
        v.push(0x4c);
        v.push(n as u8);
        v.extend_from_slice(data);
        v
    } else {
        let mut v = Vec::with_capacity(3 + n);
        v.push(0x4d);
        v.push((n & 0xff) as u8);
        v.push((n >> 8) as u8);
        v.extend_from_slice(data);
        v
    }
}

/// Build a P2PKH scriptPubKey: `OP_DUP OP_HASH160 <hash160(pubkey)> OP_EQUALVERIFY OP_CHECKSIG`.
pub fn p2pkh_script_pubkey(pubkey: &[u8; 33]) -> Vec<u8> {
    let hash = zcash_transparent::util::hash160::hash(pubkey);
    let mut s = Vec::with_capacity(25);
    s.push(0x76); // OP_DUP
    s.push(0xa9); // OP_HASH160
    s.push(0x14); // push 20
    s.extend_from_slice(&hash);
    s.push(0x88); // OP_EQUALVERIFY
    s.push(0xac); // OP_CHECKSIG
    s
}

/// Build a P2SH scriptPubKey: `OP_HASH160 <hash160(redeem_script)> OP_EQUAL`.
pub fn p2sh_script_pubkey(redeem: &[u8]) -> Vec<u8> {
    let hash = zcash_transparent::util::hash160::hash(redeem);
    let mut s = Vec::with_capacity(23);
    s.push(0xa9); // OP_HASH160
    s.push(0x14); // push 20
    s.extend_from_slice(&hash);
    s.push(0x87); // OP_EQUAL
    s
}

/// Build an OP_RETURN scriptPubKey: `OP_RETURN <push(payload)>`.
pub fn op_return_script(payload: &[u8]) -> Vec<u8> {
    let mut s = Vec::with_capacity(1 + payload.len() + 1);
    s.push(0x6a); // OP_RETURN
    s.extend_from_slice(&push_data(payload));
    s
}

/// Build a carrier scriptSig:
/// `PUSH(chunk_index) PUSH(chunk_data...)* PUSH(sig) PUSH(redeem)`.
///
/// `chunk_data` is split into ≤520-byte pushes (Zcash per-push limit).
pub fn carrier_script_sig(
    chunk_index: u8,
    chunk_data: &[u8],
    signature: &[u8], // DER sig + sighash byte
    redeem: &[u8],
) -> Vec<u8> {
    let mut s = Vec::new();
    s.extend_from_slice(&push_data(&[chunk_index]));
    for part in chunk_data.chunks(MAX_PUSH_SIZE) {
        s.extend_from_slice(&push_data(part));
    }
    s.extend_from_slice(&push_data(signature));
    s.extend_from_slice(&push_data(redeem));
    s
}

/// Build a standard P2PKH scriptSig: `PUSH(sig) PUSH(pubkey)`.
pub fn p2pkh_script_sig(signature: &[u8], pubkey: &[u8; 33]) -> Vec<u8> {
    let mut s = Vec::new();
    s.extend_from_slice(&push_data(signature));
    s.extend_from_slice(&push_data(pubkey));
    s
}

// ── Key management ───────────────────────────────────────────────────────────

/// A transparent signing key (secp256k1).
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

    /// A deterministic development key (NOT for production).
    pub fn dev_key() -> Self {
        Self::from_secret_bytes([0x11; 32]).expect("valid dev key")
    }

    pub fn compressed_pubkey(&self) -> [u8; 33] {
        self.public.serialize()
    }

    /// The P2PKH address for this key (funding source for CALL / PREPARE).
    pub fn p2pkh_address(&self) -> TransparentAddress {
        TransparentAddress::from_pubkey(&self.public)
    }
}

// ── Transparent transaction construction ─────────────────────────────────────

/// How a transparent input is spent.
pub enum SpendKind {
    /// Standard P2PKH spend (funding inputs for PREPARE/CALL).
    P2pkh { key: SigningKey },
    /// Carrier P2SH spend carrying one WASM chunk.
    Carrier {
        key: SigningKey,
        chunk_index: u8,
        chunk_data: Vec<u8>,
        redeem_script: Vec<u8>,
    },
}

/// A single transparent input to spend.
pub struct SpendInput {
    pub outpoint: OutPoint,
    pub value: u64,
    /// The spent output's scriptPubKey (P2PKH or P2SH).
    pub script_pubkey: Vec<u8>,
    /// The sighash script_code: redeem script for P2SH, scriptPubKey for P2PKH.
    pub script_code: Vec<u8>,
    pub kind: SpendKind,
}

/// A single transparent output to create.
pub struct SpendOutput {
    pub value: u64,
    pub script_pubkey: Vec<u8>,
}

/// A signed, serialized transparent transaction.
pub struct SignedTx {
    pub bytes: Vec<u8>,
    pub txid: [u8; 32],
}

impl SignedTx {
    /// txid in the display (byte-reversed) order used by Zcash RPCs/explorers.
    pub fn txid_hex(&self) -> String {
        let mut rev = self.txid;
        rev.reverse();
        hex::encode(rev)
    }
}

/// An [`Authorization`] marker whose transparent bundle is the builder's
/// [`zcash_transparent::builder::Unauthorized`] type. That type exposes the
/// input amounts and scriptPubKeys (the effecting data) required to compute the
/// ZIP-244 sighash, while holding placeholder (empty) scriptSigs.
struct TransparentUnauthorized;

impl Authorization for TransparentUnauthorized {
    type TransparentAuth = zcash_transparent::builder::Unauthorized;
    type SaplingAuth = sapling_crypto::bundle::Authorized;
    type OrchardAuth = orchard::bundle::Authorized;
}

/// Build and sign a transparent-only V5 transaction.
///
/// Uses the canonical ZIP-244 sighash (librustzcash
/// [`zcash_primitives::transaction::sighash::signature_hash`]) and the ZIP-244
/// transaction id, so the txid commits to the transaction's *effecting* data
/// (outputs + effects) and excludes authorization bytes (scriptSigs). The
/// `branch_id` is the consensus branch id active at the target height; for V5 it
/// is serialized in the transaction header and must be a Nu5+ branch.
pub fn build_transparent_tx(
    inputs: &[SpendInput],
    outputs: &[SpendOutput],
    lock_time: u32,
    branch_id: BranchId,
) -> Result<SignedTx> {
    if inputs.is_empty() {
        bail!("transaction has no transparent inputs");
    }

    let secp = secp256k1::Secp256k1::new();

    let vout: Vec<TxOut> = outputs
        .iter()
        .map(|o| {
            Ok(TxOut::new(
                Zatoshis::from_u64(o.value)?,
                Script(Code(o.script_pubkey.clone())),
            ))
        })
        .collect::<Result<_>>()?;

    // 1. Build the `Unauthorized` bundle (placeholder scriptSigs + the input
    //    amounts/scriptPubKeys needed for the ZIP-244 sighash). The canonical
    //    `TransparentBuilder` validates P2PKH/P2SH spend info against the coin.
    let mut builder = TransparentBuilder::empty();
    for inp in inputs {
        let coin = TxOut::new(
            Zatoshis::from_u64(inp.value)?,
            Script(Code(inp.script_pubkey.clone())),
        );
        match &inp.kind {
            SpendKind::P2pkh { key } => {
                builder.add_p2pkh_input(key.public, inp.outpoint.clone(), coin)?;
            }
            SpendKind::Carrier { redeem_script, .. } => {
                let redeem = zcash_script::script::FromChain::parse(&Code(redeem_script.clone()))
                    .map_err(|e| anyhow::anyhow!("invalid redeem script: {e}"))?;
                builder.add_p2sh_input(redeem, inp.outpoint.clone(), coin)?;
            }
        }
    }
    let mut bundle_unauth = builder
        .build()
        .ok_or_else(|| anyhow::anyhow!("empty transaction"))?;
    // The builder cannot add OP_RETURN (nulldata) outputs; overwrite `vout` with
    // the exact outputs after `build()`.
    bundle_unauth.vout = vout.clone();

    let tx_placeholder: TransactionData<TransparentUnauthorized> = TransactionData::from_parts(
        TxVersion::V5,
        branch_id,
        lock_time,
        BlockHeight::from_u32(0),
        Some(bundle_unauth.clone()),
        None,
        None,
        None,
    );

    // ZIP-244 txid parts (digests of the effecting data). These are shared by
    // every input's signature hash and by the final transaction id.
    let txid_parts = tx_placeholder.digest(TxIdDigester);

    // 2. Sighash + sign each input.
    let mut final_script_sigs = Vec::with_capacity(inputs.len());
    for (i, inp) in inputs.iter().enumerate() {
        let script_code_script = Script(Code(inp.script_code.clone()));
        let script_pubkey_script = Script(Code(inp.script_pubkey.clone()));
        let value = Zatoshis::from_u64(inp.value)?;

        let signable = TransparentSignableInput::from_parts(
            &bundle_unauth,
            SighashType::ALL,
            i,
            &script_code_script,
            &script_pubkey_script,
            value,
        )
        .map_err(|e| anyhow::anyhow!("invalid input index: {e}"))?;

        let sighash = signature_hash(
            &tx_placeholder,
            &PrimitivesSignableInput::Transparent(signable),
            &txid_parts,
        );
        let mut msg = [0u8; 32];
        msg.copy_from_slice(sighash.as_ref());
        let msg = secp256k1::Message::from_digest(msg);

        let script_sig = match &inp.kind {
            SpendKind::P2pkh { key } => {
                let sig = secp.sign_ecdsa(&msg, &key.secret);
                let mut sig_bytes = sig.serialize_der().to_vec();
                sig_bytes.push(SIGHASH_ALL);
                p2pkh_script_sig(&sig_bytes, &key.compressed_pubkey())
            }
            SpendKind::Carrier {
                key,
                chunk_index,
                chunk_data,
                redeem_script,
            } => {
                let sig = secp.sign_ecdsa(&msg, &key.secret);
                let mut sig_bytes = sig.serialize_der().to_vec();
                sig_bytes.push(SIGHASH_ALL);
                carrier_script_sig(*chunk_index, chunk_data, &sig_bytes, redeem_script)
            }
        };
        final_script_sigs.push(script_sig);
    }

    // 3. Rebuild with real scriptSigs, freeze, serialize.
    let vin: Vec<TxIn<TAuthorized>> = inputs
        .iter()
        .zip(final_script_sigs)
        .map(|(inp, sig)| TxIn::from_parts(inp.outpoint.clone(), Script(Code(sig)), u32::MAX))
        .collect();
    let bundle = Bundle {
        vin,
        vout,
        authorization: TAuthorized,
    };
    let tx_data: TransactionData<Authorized> = TransactionData::from_parts(
        TxVersion::V5,
        branch_id,
        lock_time,
        BlockHeight::from_u32(0),
        Some(bundle),
        None,
        None,
        None,
    );
    let tx: Transaction = tx_data.freeze()?;

    let mut bytes = Vec::new();
    tx.write(&mut bytes)?;

    let mut txid = [0u8; 32];
    txid.copy_from_slice(tx.txid().as_ref());

    Ok(SignedTx { bytes, txid })
}

// ── High-level PREPARE / DEPLOY / CALL ───────────────────────────────────────

/// Serialized size of a script (CompactSize length prefix + bytes).
fn serialized_script_size(len: usize) -> usize {
    let compact = if len < 0xfd {
        1
    } else if len <= 0xffff {
        3
    } else {
        5
    };
    compact + len
}

/// Serialized transparent input size: prevout(36) + scriptSig + sequence(4).
fn input_serialized_size(script_sig_len: usize) -> usize {
    36 + serialized_script_size(script_sig_len) + 4
}

/// Serialized transparent output size: value(8) + scriptPubKey.
fn output_serialized_size(script_pubkey_len: usize) -> usize {
    8 + serialized_script_size(script_pubkey_len)
}

/// Length of a P2PKH scriptSig: PUSH(sig≤73) + PUSH(pubkey=33).
fn p2pkh_script_sig_len() -> usize {
    (1 + MAX_SIGNATURE_LEN) + (1 + 33)
}

/// Length of a carrier scriptSig for a given chunk payload.
fn carrier_script_sig_len(chunk_len: usize) -> usize {
    // chunk_index push (2) + chunk data pushes + sig push + redeem push.
    let mut len = 2; // chunk_index
    let mut remaining = chunk_len;
    while remaining > 0 {
        let part = remaining.min(MAX_PUSH_SIZE);
        len += push_overhead(part) + part;
        remaining -= part;
    }
    len += 1 + MAX_SIGNATURE_LEN; // sig push
    len += push_overhead(REDEEM_SCRIPT_LEN) + REDEEM_SCRIPT_LEN; // redeem push
    len
}

/// Split WASM bytes into canonical chunks of `CHUNK_PAYLOAD_SIZE`.
pub fn split_chunks(wasm: &[u8]) -> Result<Vec<Vec<u8>>> {
    let n = carrier_input_count(wasm.len())?;
    let mut chunks = Vec::with_capacity(n as usize);
    for part in wasm.chunks(CHUNK_PAYLOAD_SIZE) {
        chunks.push(part.to_vec());
    }
    Ok(chunks)
}

/// Build the PREPARE transaction: spend one or more funding UTXOs, create N
/// P2SH carrier outputs, and return change to the funding address.
pub fn build_prepare(
    funding_key: &SigningKey,
    funding_utxos: &[(OutPoint, u64)],
    carrier_values: &[u64],
    branch_id: BranchId,
) -> Result<SignedTx> {
    if funding_utxos.is_empty() {
        bail!("no funding UTXOs");
    }
    let pubkey = funding_key.compressed_pubkey();
    let redeem = redeem_script(&pubkey);
    let carrier_script = p2sh_script_pubkey(&redeem);

    let carrier_total: u64 = carrier_values.iter().sum();
    let funding_total: u64 = funding_utxos.iter().map(|(_, v)| v).sum();

    let in_size: usize = funding_utxos
        .iter()
        .map(|_| input_serialized_size(p2pkh_script_sig_len()))
        .sum();
    let out_size: usize = carrier_values
        .iter()
        .map(|_| output_serialized_size(carrier_script.len()))
        .sum::<usize>()
        + output_serialized_size(p2pkh_script_pubkey(&pubkey).len());
    let fee = zip317_fee_from_sizes(in_size, out_size);

    let change = funding_total
        .checked_sub(carrier_total)
        .and_then(|v| v.checked_sub(fee))
        .ok_or_else(|| anyhow::anyhow!("funding UTXOs too small for PREPARE"))?;

    let mut outputs = Vec::with_capacity(carrier_values.len() + 1);
    for v in carrier_values {
        outputs.push(SpendOutput {
            value: *v,
            script_pubkey: carrier_script.clone(),
        });
    }
    if change > 0 {
        outputs.push(SpendOutput {
            value: change,
            script_pubkey: p2pkh_script_pubkey(&pubkey),
        });
    }

    let inputs: Vec<SpendInput> = funding_utxos
        .iter()
        .map(|(outpoint, value)| SpendInput {
            outpoint: outpoint.clone(),
            value: *value,
            script_pubkey: p2pkh_script_pubkey(&pubkey),
            script_code: p2pkh_script_pubkey(&pubkey),
            kind: SpendKind::P2pkh {
                key: funding_key.clone(),
            },
        })
        .collect();

    build_transparent_tx(&inputs, &outputs, 0, branch_id)
}

/// Build the DEPLOY transaction: spend N carrier UTXOs with WASM chunks in the
/// scriptSigs, one OP_RETURN output carrying the Zalkanes DEPLOY message, and a
/// change output returning the excess carrier value to the deployer.
pub fn build_deploy(
    key: &SigningKey,
    carrier_outpoints: &[OutPoint],
    carrier_values: &[u64],
    chunks: &[Vec<u8>],
    op_return: &[u8],
    branch_id: BranchId,
) -> Result<SignedTx> {
    if carrier_outpoints.len() != chunks.len() || carrier_outpoints.len() != carrier_values.len() {
        bail!("carrier outpoints/values/chunks length mismatch");
    }
    let pubkey = key.compressed_pubkey();
    let redeem = redeem_script(&pubkey);
    let carrier_script = p2sh_script_pubkey(&redeem);

    let in_size: usize = chunks
        .iter()
        .map(|c| input_serialized_size(carrier_script_sig_len(c.len())))
        .sum();
    let op_return = op_return_script(op_return);
    let op_return_script_len = op_return.len();
    let out_size = output_serialized_size(op_return_script_len)
        + output_serialized_size(p2pkh_script_pubkey(&pubkey).len());
    let fee = zip317_fee_from_sizes(in_size, out_size);

    let carrier_total: u64 = carrier_values.iter().sum();
    let change = carrier_total
        .checked_sub(fee)
        .ok_or_else(|| anyhow::anyhow!("carrier values too small for DEPLOY fee"))?;

    let mut inputs = Vec::with_capacity(carrier_outpoints.len());
    for (i, ((outpoint, value), chunk)) in carrier_outpoints
        .iter()
        .zip(carrier_values.iter())
        .zip(chunks.iter())
        .enumerate()
    {
        inputs.push(SpendInput {
            outpoint: outpoint.clone(),
            value: *value,
            script_pubkey: carrier_script.clone(),
            script_code: redeem.clone(),
            kind: SpendKind::Carrier {
                key: key.clone(),
                chunk_index: i as u8,
                chunk_data: chunk.clone(),
                redeem_script: redeem.clone(),
            },
        });
    }

    let mut outputs = vec![SpendOutput {
        value: 0,
        script_pubkey: op_return,
    }];
    if change > 0 {
        outputs.push(SpendOutput {
            value: change,
            script_pubkey: p2pkh_script_pubkey(&pubkey),
        });
    }

    build_transparent_tx(&inputs, &outputs, 0, branch_id)
}

/// Build a CALL transaction: spend one P2PKH funding UTXO, emit an OP_RETURN
/// carrying the Zalkanes CALL message, and return change.
pub fn build_call(
    funding_key: &SigningKey,
    funding_outpoint: OutPoint,
    funding_value: u64,
    op_return: &[u8],
    branch_id: BranchId,
) -> Result<SignedTx> {
    let pubkey = funding_key.compressed_pubkey();
    let op_return = op_return_script(op_return);
    let in_size = input_serialized_size(p2pkh_script_sig_len());
    let out_size = output_serialized_size(op_return.len())
        + output_serialized_size(p2pkh_script_pubkey(&pubkey).len());
    let fee = zip317_fee_from_sizes(in_size, out_size);

    let change = funding_value
        .checked_sub(fee)
        .ok_or_else(|| anyhow::anyhow!("funding UTXO too small for CALL"))?;

    build_transparent_tx(
        &[SpendInput {
            outpoint: funding_outpoint,
            value: funding_value,
            script_pubkey: p2pkh_script_pubkey(&pubkey),
            script_code: p2pkh_script_pubkey(&pubkey),
            kind: SpendKind::P2pkh {
                key: funding_key.clone(),
            },
        }],
        &[
            SpendOutput {
                value: 0,
                script_pubkey: op_return,
            },
            SpendOutput {
                value: change,
                script_pubkey: p2pkh_script_pubkey(&pubkey),
            },
        ],
        0,
        branch_id,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redeem_script_shape() {
        let k = SigningKey::dev_key();
        let pk = k.compressed_pubkey();
        let r = redeem_script(&pk);
        assert_eq!(r.len(), REDEEM_SCRIPT_LEN);
        assert_eq!(r[0], 0x21);
        assert_eq!(r[34], 0xac); // OP_CHECKSIG
        assert_eq!(r[35], 0x61); // OP_NOP
    }

    #[test]
    fn chunk_size_is_within_policy() {
        let k = SigningKey::dev_key();
        let pk = k.compressed_pubkey();
        let redeem = redeem_script(&pk);
        let sig = vec![0x30; MAX_SIGNATURE_LEN];
        let chunk = vec![0xAA; CHUNK_PAYLOAD_SIZE];
        let ss = carrier_script_sig(0, &chunk, &sig, &redeem);
        assert!(
            ss.len() <= MAX_STANDARD_SCRIPTSIG_SIZE,
            "scriptSig {} > {}",
            ss.len(),
            MAX_STANDARD_SCRIPTSIG_SIZE
        );
    }

    #[test]
    fn max_payload_derivation_is_consistent() {
        let budget = max_standard_carrier_payload(MAX_SIGNATURE_LEN, REDEEM_SCRIPT_LEN);
        assert!(CHUNK_PAYLOAD_SIZE + 3 <= budget);
    }

    #[test]
    fn carrier_input_count_matches() {
        assert_eq!(carrier_input_count(1).unwrap(), 1);
        assert_eq!(carrier_input_count(CHUNK_PAYLOAD_SIZE).unwrap(), 1);
        assert_eq!(carrier_input_count(CHUNK_PAYLOAD_SIZE + 1).unwrap(), 2);
        assert!(carrier_input_count(0).is_err());
    }
}
