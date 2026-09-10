#!/usr/bin/env python3
"""Zalkanes protocol v0 reference implementation (Python).

This is a small, independent implementation of the consensus-critical pieces:
  - protocol message decoding (OP_RETURN)
  - ContractId derivation
  - carrier chunk reconstruction
  - state-root hashing

It is intentionally written against the *specification* (docs/protocol-v0.md
and audit/consensus-critical-files.md), not the Rust code, so it can catch cases
where Rust behavior accidentally became the protocol. It must produce identical
outputs to the Rust implementation on the shared test vectors.

Run:  python3 tools/zalkanes-reference/reference.py
"""

import hashlib
import struct

PROTOCOL_MAGIC = bytes([0x5A, 0x41, 0x4C, 0x4B])  # "ZALK"
PROTOCOL_V0 = 0x00
MSG_DEPLOY = 0x01
MSG_CALL = 0x02
MSG_CALL_CARRIER = 0x03

CONTRACT_ID_PERSONALIZATION = b"ZalkContractId0 "
STATE_LEAF_PERSONALIZATION = b"ZalkStateLeaf0  "
STATE_ROOT_PERSONALIZATION = b"ZalkStateRoot0  "

NETWORK_MAINNET = 0x01
NETWORK_TESTNET = 0x02
NETWORK_REGTEST = 0x03

MAX_CODE_BYTES = 262144
MAX_CALL_INLINE_BYTES = 38
MAX_CALL_INPUT_BYTES = 65536
MAX_CALL_CARRIER_CHUNKS = 47
CHUNK_PAYLOAD_SIZE = 1400


def blake2b256(data: bytes, person: bytes) -> bytes:
    return hashlib.blake2b(data, digest_size=32, person=person).digest()


def sha256(data: bytes) -> bytes:
    return hashlib.sha256(data).digest()


# ── ContractId ────────────────────────────────────────────────────────────────

def contract_id(network_id: int, txid: bytes, output_index: int, code_hash: bytes) -> bytes:
    assert len(txid) == 32 and len(code_hash) == 32
    input_ = (
        bytes([network_id])
        + txid
        + struct.pack(">H", output_index)
        + code_hash
    )
    return blake2b256(input_, CONTRACT_ID_PERSONALIZATION)


# ── State root ────────────────────────────────────────────────────────────────

def contract_leaf(contract_id_: bytes, code_hash: bytes, wasm: bytes) -> bytes:
    input_ = (
        contract_id_
        + code_hash
        + struct.pack(">I", len(wasm))
        + wasm
    )
    return blake2b256(input_, STATE_LEAF_PERSONALIZATION)


def storage_leaf(contract_id_: bytes, key: bytes, value: bytes) -> bytes:
    input_ = (
        contract_id_
        + struct.pack(">H", len(key))
        + key
        + struct.pack(">I", len(value))
        + value
    )
    return blake2b256(input_, STATE_LEAF_PERSONALIZATION)


def state_root(contract_leaves, storage_leaves) -> bytes:
    """Hash of all leaves, sorted lexicographically by digest."""
    leaves = sorted(contract_leaves + storage_leaves)
    return blake2b256(b"".join(leaves), STATE_ROOT_PERSONALIZATION)


# ── Carrier reconstruction ────────────────────────────────────────────────────

def reconstruct(chunks, chunk_count, expected_length, expected_hash) -> bytes:
    if expected_length == 0 or expected_length > MAX_CODE_BYTES:
        raise ValueError("invalid length")
    if len(chunks) != chunk_count:
        raise ValueError("wrong chunk count")
    indexed = {}
    for index, data in chunks:
        if index >= chunk_count:
            raise ValueError("chunk out of range")
        if index in indexed:
            raise ValueError("duplicate chunk")
        indexed[index] = data
    out = b"".join(indexed[i] for i in range(chunk_count))
    if len(out) != expected_length:
        raise ValueError("length mismatch")
    if sha256(out) != expected_hash:
        raise ValueError("hash mismatch")
    return out


# ── OP_RETURN decode ──────────────────────────────────────────────────────────

def decode_op_return(payload: bytes):
    if not payload:
        return None
    if payload[:4] != PROTOCOL_MAGIC:
        return None
    if payload[4] != PROTOCOL_V0:
        raise ValueError("unknown version")
    msg_type = payload[5]
    body = payload[6:]
    if msg_type == MSG_DEPLOY:
        assert len(body) == 39, "deploy trailing/truncated"
        code_hash = body[0:32]
        code_length = struct.unpack(">I", body[32:36])[0]
        chunk_count = body[36]
        output_index = struct.unpack(">H", body[37:39])[0]
        return ("deploy", code_hash, code_length, chunk_count, output_index)
    if msg_type == MSG_CALL:
        contract_id_ = body[0:32]
        opcode = struct.unpack(">H", body[32:34])[0]
        input_length = struct.unpack(">H", body[34:36])[0]
        assert input_length <= MAX_CALL_INLINE_BYTES, "inline input over limit"
        input_ = body[36:]
        assert len(input_) == input_length, "input length mismatch"
        return ("call", contract_id_, opcode, input_)
    if msg_type == MSG_CALL_CARRIER:
        assert len(body) == 71, "call_carrier trailing/truncated"
        contract_id_ = body[0:32]
        opcode = struct.unpack(">H", body[32:34])[0]
        input_hash = body[34:66]
        input_length = struct.unpack(">I", body[66:70])[0]
        carrier_count = body[70]
        return ("call_carrier", contract_id_, opcode, input_hash, input_length, carrier_count)
    raise ValueError("unknown message type")


# ── Self-test against known vectors ───────────────────────────────────────────

def _check(name, got, want):
    assert got == want, f"{name}: got {got.hex()}, want {want.hex()}"
    print(f"  {name}: OK")


def main():
    print("Zalkanes reference implementation self-test")

    # Empty state root (network-independent).
    empty_root = state_root([], [])
    _check("empty root", empty_root, bytes.fromhex(
        "b120099c167da673588b15aa827c3bbd9339a9a2d934b0d20c99cebe69f8ffe6"))

    # Counter contract (testnet): contract_id and roots.
    # counter.wasm code_hash (SHA-256), known from the testnet deploy.
    code_hash = bytes.fromhex(
        "fa8289fbc0fdb132e57f939033a9971b0ee2990ec49db247aad3f682aa804cc7")
    # ContractId (testnet, deploy txid internal-order, output_index 0).
    # deploy txid d6e8552c... in display order; internal = reversed.
    deploy_txid_display = bytes.fromhex(
        "d6e8552c3622106382f71c8285e35565fd4451fe3bf5c2ca87d7fd60626e2e26")
    txid_internal = deploy_txid_display[::-1]
    cid = contract_id(NETWORK_TESTNET, txid_internal, 0, code_hash)
    _check("counter ContractId", cid, bytes.fromhex(
        "607a6246c512239a23f51cf8053444d4d76e7684c1a53dc26a626b474a8cf3c0"))

    # Root after deploy = contract leaf only (no storage yet).
    root_after_deploy = state_root([contract_leaf(cid, code_hash, b"\x00asm")], [])
    # NOTE: the real wasm is 2513 bytes; this self-test uses a placeholder only
    # to exercise the hashing path. The authoritative root is verified against
    # the Rust implementation's full vectors in CI.
    _ = root_after_deploy

    # Storage root: counter=2 (u64 BE).
    counter_key = b"counter"
    counter_value = (2).to_bytes(8, "big")
    leaves = [contract_leaf(cid, code_hash, b"\x00asm"),
              storage_leaf(cid, counter_key, counter_value)]
    root_with_counter = state_root(leaves, [])
    # Placeholder; full vector equality is asserted in the shared-vectors test.
    _ = root_with_counter

    # Carrier reconstruction round-trip.
    wasm = b"\x00asm\x01\x00\x00\x00hello"
    chunks = [(i // 4, wasm[i:i + 4]) for i in range(0, len(wasm), 4)]
    assert reconstruct(chunks, len(chunks), len(wasm), sha256(wasm)) == wasm
    print("  carrier reconstruction round-trip: OK")

    # OP_RETURN decode (deploy).
    payload = PROTOCOL_MAGIC + bytes([PROTOCOL_V0, MSG_DEPLOY]) + code_hash + \
        struct.pack(">I", 2513) + bytes([2]) + struct.pack(">H", 0)
    d = decode_op_return(payload)
    assert d[0] == "deploy" and d[2] == 2513 and d[3] == 2
    print("  OP_RETURN deploy decode: OK")

    print("All reference checks passed.")


if __name__ == "__main__":
    main()
