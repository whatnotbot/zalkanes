#!/usr/bin/env python3
"""Generate permanent state-root test vectors.

Produces 100 deterministic vectors: for N in 1..=100, the state root of a store
containing one deployed contract and one storage entry
(key = "key<N>", value = 8-byte big-endian N). The contract id, code hash, and
wasm are fixed, so the vectors are reproducible across languages.

Writes JSON to stdout: a list of {"n": N, "root": "<hex>"}.
"""

import json
import sys

sys.path.insert(0, __file__.rsplit("/", 1)[0])
from reference import contract_leaf, storage_leaf, state_root  # noqa: E402

FIXED_CONTRACT_ID = bytes(range(32))       # 0x00..0x1f
FIXED_CODE_HASH = bytes([0xAB] * 32)
FIXED_WASM = b"\x00asm\x01\x00\x00\x00"   # minimal wasm header


def main():
    vectors = []
    for n in range(1, 101):
        key = f"key{n}".encode()
        value = n.to_bytes(8, "big")
        leaves = [
            contract_leaf(FIXED_CONTRACT_ID, FIXED_CODE_HASH, FIXED_WASM),
            storage_leaf(FIXED_CONTRACT_ID, key, value),
        ]
        vectors.append({"n": n, "root": state_root(leaves, []).hex()})
    print(json.dumps(vectors, indent=2))


if __name__ == "__main__":
    main()
