# ADR 0003 — Transaction Carrier Design

**Status:** Accepted
**Date:** 2025-09-10 (validated against live Zebra v6.3.0 regtest)

## Context

Zcash standard OP_RETURN outputs are limited to 80 bytes. WASM binaries for
non-trivial contracts easily exceed this. The WASM bytes must be recoverable
entirely from Zcash blockchain data with no external dependencies (no IPFS,
Arweave, HTTP, CDN, or project API).

## Design: two-stage P2SH carrier

### Stage 1 — PREPARE transaction

The deployer constructs N P2SH carrier UTXOs, one per WASM chunk. Each UTXO's
redeem script is the **non-standard** script:

```
<deployer_pubkey(33 bytes)> OP_CHECKSIG OP_NOP
```

Serialized redeem script (36 bytes): `0x21 <pubkey> 0xac 0x61`.

This script is:

1. not anyone-can-spend — requires the deployer's signature (OP_CHECKSIG);
2. **non-standard** as a solver template (the trailing `OP_NOP` makes
   `solver::standard()` return `None`, so it does NOT match the P2PK template);
3. consensus-valid — executes successfully, leaving `true` on top;
4. 1 sigop (OP_CHECKSIG) ≤ the 15-sigop P2SH standardness limit.

The P2SH scriptPubKey is the standard `OP_HASH160 <hash160(redeem_script)> OP_EQUAL`.

### Stage 2 — DEPLOY transaction

The DEPLOY transaction spends the N carrier UTXOs. Each scriptSig is push-only
and encodes, in order:

```
PUSH(chunk_index)        # 1 byte, 0..N-1
PUSH(chunk_data...)      # chunk bytes, split into ≤520-byte pushes
PUSH(signature)          # DER ECDSA sig + SIGHASH_ALL byte
PUSH(redeem_script)      # the 36-byte redeem script above
```

The DEPLOY transaction also includes an OP_RETURN output with the Zalkanes
DEPLOY message:

```
ZALK (4 bytes) || version (1) || DEPLOY type (1)
|| code_hash (32) || code_length (u32) || chunk_count (u8) || output_index (u16)
```

### Why the non-standard redeem script is required

For a **standard** P2SH→P2PK spend, Zebra's `are_inputs_standard` computes the
expected scriptSig argument count and rejects any extra pushes. The trailing
`OP_NOP` makes the redeem script non-standard, so Zebra's solver classifies it
as `None`, and `are_inputs_standard` instead applies the branch:

> "Any other Script with less than 15 sigops OK: ... extra data left on the
> stack after execution is OK, too"

(Zebra `zebra-consensus/src/transaction/check.rs`, mirroring zcashd's
`AreInputsStandard`.) This is what permits the extra `chunk_index` and
`chunk_data` pushes below the final `true` value.

## Empirical validation

Tested against the **unmodified live Zebra v6.3.0 regtest** with the internal
miner and default standardness (no policy relaxation, no direct block mining
around a rejected mempool tx):

| payload | scriptSig bytes | result |
|---------|-----------------|--------|
| 512 B   | 628             | accepted + mined + reconstructed |
| 520 B   | 636             | accepted + mined + reconstructed |
| 1024 B (single push) | n/a | REJECTED: `PushSize` (per-push 520-byte limit) |
| 1400 B (multi-push) | 1522 | accepted + mined + reconstructed |
| 65536 B (47 carriers) | — | accepted + mined + reconstructed |
| 262144 B (188 carriers) | — | accepted + mined + reconstructed |

A single script push is capped at 520 bytes (`MAX_SCRIPT_ELEMENT_SIZE`). Chunk
data must therefore be split into multiple ≤520-byte pushes within the scriptSig.
Multi-carrier deployments scale to the protocol maximum (255 chunks).

## Chunk size

`MAX_STANDARD_SCRIPTSIG_SIZE = 1650` bytes. The fixed per-carrier-input
overhead is:

```
chunk_index push        = 2 bytes
signature push          = 1 + 73 = 74 bytes (max DER sig + sighash byte)
redeem_script push      = 1 + 36 = 37 bytes
```

leaving 1537 bytes. With per-push overhead (3 bytes per 520-byte push), the
practical maximum is 1466 bytes. We freeze:

```
CHUNK_PAYLOAD_SIZE = 1400 bytes
```

giving ~66 bytes of headroom for DER signature variance.

Maximum v0 contract size = 255 chunks × 1400 bytes = **357,000 bytes (≈348 KiB)**.

## ZIP-244 / txid implications

Zalkanes builds **version 4** transparent transactions and signs them with the
legacy ZIP-243 sighash. V4 remains consensus-valid on every current network
upgrade (Zebra's `verify_v4_transaction_network_upgrade` accepts V4 through
Nu6.3). The ZIP-243 sighash personalization embeds a **consensus branch id**,
and the validator recomputes it from the network upgrade active at the spending
height (`NetworkUpgrade::current(network, height).branch_id()`), so the branch
id is per-network and height-dependent, not a constant:

| Network          | Branch id   | Upgrade |
|------------------|-------------|---------|
| regtest (Zebra)  | `0xE9FF75A6` | Canopy  |
| testnet (current)| `0x37A5165B` | Nu6.3   |
| mainnet          | *(unset)*   | pre-audit |

`zalkanes_tx::branch_id_for_network` pins this mapping. The carrier mechanism
itself (redeem script + scriptSig encoding) is network-independent and unchanged;
only the sighash branch id differs per network. The `code_hash` in the OP_RETURN
output commits the effecting data; reconstruction verifies
`SHA-256(reconstructed) == code_hash`, rejecting any carrier mutation.

## Fee behavior

ZIP-317 conventional fee (5000 zatoshi per logical action, minimum 2).

## Acceptance criteria

All of CARRIER-001 through CARRIER-011 must pass; the live-relay sequence
(PREPARE → mine → DEPLOY → sendrawtransaction → mine → reconstruct) is
encoded in `crates/zalkanes-tx/examples/carrier_relay.rs` and was executed
against the live Zebra regtest (see validation table above).

## Consequences

- Two on-chain transactions per deployment (PREPARE + DEPLOY).
- Deployer must hold ZEC to fund both transactions.
- Maximum contract size bounded to ~348 KiB by chunk count (255) × chunk size (1400 B).
- Chunk ordering is explicit via `chunk_index`; reconstruction is deterministic.
