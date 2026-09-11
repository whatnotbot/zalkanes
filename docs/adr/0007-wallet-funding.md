# ADR 0007 — Wallet Funding Layer (Transparent + Shielded)

**Status:** Accepted
**Date:** 2025-09-11 (API study against librustzcash `pczt` 0.9.3,
`zcash_client_backend` 0.24.0, `zcash_client_sqlite` 0.22.0, and Zallet's
generated JSON-RPC reference)

## Context

Milestone 4 requires the deploy/call path to be fundable from **either** the
transparent pool **or** the shielded pool (Orchard and/or Ironwood), with the
following hard requirements:

1. The canonical ZALK message (OP_RETURN payload + P2SH carrier structure) must
   be **byte-identical** regardless of which pool funds the transaction.
2. Shielded change is preferred; transparent change is a fallback, not a
   default.
3. No seeds/keys may be stored in RocksDB (the state DB is consensus data only).
4. No wallet methods may be exposed on the public `zalkanes-rpc` JSON-RPC.
5. We must not claim the smart-contract *execution* is private: the ZALK message
   and carrier are on-chain in cleartext. Only the *funding/change* side gains
   shielded-pool privacy.

Before writing code we studied the current upstream APIs. The decisive finding
is that the **high-level wallet entry points cannot express the Zalkanes
transparent bundle**, and we must therefore construct the transaction at the
lower canonical layer. This ADR records that finding and the chosen design.

## Upstream API study

### Zallet's high-level RPC cannot express the ZALK bundle

Zallet's PCZT pipeline is exposed as JSON-RPC methods `pczt_create`,
`pczt_combine`, `pczt_inspect`, `pczt_prove`, `pczt_sign`, `pczt_extract`
([Zallet JSON-RPC reference](https://zcash.github.io/zallet/rpc/index.html)).
`pczt_create` takes a `from` (address or account UUID) and an `amounts` array of
`{ address, amount, memo }` recipients. Recipients are **Zcash addresses only**;
there is no way to specify an arbitrary `scriptPubKey` (OP_RETURN or P2SH
carrier). `pczt_create` therefore cannot produce the Zalkanes PREPARE (N P2SH
carrier outputs) or DEPLOY/CALL (OP_RETURN ZALK message) transactions.

### `zcash_client_backend`'s proposal API has the same limitation

`zcash_client_backend::data_api::wallet::create_pczt_from_proposal` and the
underlying `Proposal::single_step` are built on
`zcash_client_backend::zip321::TransactionRequest`, whose `Payment`s are
address/memo/amount tuples. `Proposal::single_step(transaction_request, ...)`
has no channel for an arbitrary transparent `scriptPubKey`, so the ZIP-321
proposal path — and thus `create_pczt_from_proposal` — cannot inject the ZALK
OP_RETURN or carrier outputs either.

### The PCZT *format* can represent the ZALK bundle

The PCZT encoding itself is not the limiting factor: a transparent output is
`pczt::transparent::Output`, whose `script_pubkey()` is arbitrary bytes
([pczt::transparent](https://zcash.github.io/librustzcash/rustdoc/latest/pczt/transparent/index.html)).
The limitation is only in the high-level constructors, not the wire format.

### The lower canonical layer can, and is the right tool

`zcash_primitives::transaction::builder::Builder` provides exactly the
primitives needed to build a Zalkanes transaction with arbitrary funding:

- `add_transparent_null_data_output(data)` — the OP_RETURN ZALK message;
- `add_transparent_output(script, amount)` — the P2SH carrier `scriptPubKey`
  (`OP_HASH160 <hash160(redeem)> OP_EQUAL`) and transparent change;
- `add_transparent_p2pkh_input` / `add_transparent_p2sh_input` — transparent
  funding;
- `add_orchard_spend` / `add_orchard_output` / `add_orchard_change_output` and
  `add_ironwood_spend` / `add_ironwood_output` — shielded funding and shielded
  change;
- `build_for_pczt(rng, fee_rule)` → `PcztParts`, which
  `pczt::roles::creator::Creator::build_from_parts(parts)` turns into a `Pczt`
  carrying all four bundles (transparent, sapling, orchard, ironwood).

The resulting `Pczt` is then finalized with the standard roles: `Prover`
(creates Orchard/Ironwood/Sapling proofs), `Signer` (transparent signatures),
and `TxExtractor` (extract + broadcast). This is the same pipeline Zallet
implements internally; we use it directly rather than driving Zallet over RPC,
because the Zalkanes outputs must be constructed below the ZIP-321 layer.

## Version matrix

All new wallet crates resolve against the already-pinned consensus crates
(`zcash_primitives 0.30.1`, `zcash_protocol 0.10.6`, `zcash_transparent 0.10.0`,
`orchard 0.15.5`, `zcash_script 0.4.3`, `sapling-crypto 0.7.0`):

| crate                   | version | compatible with pins |
|-------------------------|---------|----------------------|
| `pczt`                  | `=0.9.3`  | `^0.30.0` / `^0.10.4` / `^0.10.0` ✓ |
| `zcash_client_backend`  | `=0.24.0` | `^0.30.1` / `^0.10.5` / `^0.10.0` ✓ |
| `zcash_client_sqlite`   | `=0.22.0` | `^0.24.0` / `^0.30.1` / `^0.15.0` ✓ |
| `zcash_keys`            | `=0.16.1` | `^0.16.1` ✓ |
| `zcash_proofs`          | `=0.30.0` | `^0.30.0` ✓ |

`zcash_client_sqlite` must be **0.22.0** (the 0.17.x line tracks
`zcash_client_backend ^0.19` and `zcash_primitives ^0.23` and would conflict
with the consensus pins).

## Decision

### `FundingSource` abstraction

`crates/zalkanes-wallet` introduces a single trait that both funding pools
implement:

```
trait FundingSource {
    fn pool_name(&self) -> &'static str;         // "transparent" | "shielded"
    fn fund(&self, request: &TxRequest, ctx: &FundContext) -> Result<SignedTx>;
}
```

`TxRequest` is the *canonical* Zalkanes transaction description
(`Prepare { carrier_values }`, `Deploy { chunks, carrier_outpoints,
carrier_values, op_return }`, `Call { op_return }`) and is **funding-pool
agnostic**. `SignedTx { bytes, txid }` is the serialized, signed transaction.
The indexer-visible ZALK payload is produced by shared code paths
(`zalkanes-tx`'s `op_return_script` / `redeem_script` / carrier scriptSig), so
the two funding sources cannot drift in what they commit on chain.

### TransparentFunding

A thin adapter over the existing `zalkanes-tx` V5/ZIP-244 builder
(`build_prepare` / `build_deploy` / `build_call`). Transparent funding spends a
P2PKH funding UTXO and returns transparent change. No new consensus code.

### ShieldedFunding (lower-level PCZT, not Zallet RPC)

Because the high-level proposal/Zallet entry points cannot express the ZALK
bundle, `ShieldedFunding` constructs the transaction with
`zcash_primitives::transaction::builder::Builder`:

1. obtain Orchard/Ironwood notes + witnesses from `zcash_client_sqlite`
   (the wallet store — SQLite, not RocksDB);
2. `add_transparent_null_data_output(zalk_message)` and
   `add_transparent_output(carrier_script, value)` for the ZALK outputs;
3. `add_orchard_spend` / `add_ironwood_spend` for funding and
   `add_*_change_output` for shielded change;
4. `build_for_pczt` → `pczt::roles::creator::Creator::build_from_parts` → `Pczt`;
5. `pczt::roles::prover::Prover::prove` → `pczt::roles::signer::Signer` →
   `pczt::roles::tx_extractor::TxExtractor::extract`.

This keeps the ZALK message byte-identical to the transparent path while letting
funding and change stay in the shielded pool. For PREPARE, value is deshielded
into the carrier P2SH outputs by construction (the carrier UTXOs must hold value
to fund the DEPLOY fee); the ZALK *message* remains identical.

### Privacy policy

Shielded funding reports a minimum privacy policy in Zallet's vocabulary
(`FullPrivacy`, `AllowRevealedAmounts`, `AllowRevealedRecipients`,
`AllowRevealedSenders`, `AllowFullyTransparent`, `AllowLinkingAccountAddresses`,
`NoPrivacy`). Because every Zalkanes transaction publishes the ZALK message and
carrier in cleartext, the minimum requirement is `AllowRevealedAmounts` (the
carrier output values reveal amounts); the CLI displays this and requires
acknowledgement before signing. We do **not** claim the contract execution is
private.

## Non-goals

- No private/encrypted smart-contract execution (out of scope, see spec §51).
- No seeds or spending keys in RocksDB.
- No wallet RPC on `zalkanes-rpc`.
- No mainnet activation.

## Consequences

- Shielded funding adds the full `zcash_client_backend`/`zcash_client_sqlite`/
  `pczt`/`zcash_proofs`/`zcash_keys` dependency subtree (including the Orchard
  proving stack). This is gated behind a `shielded` cargo feature so the
  consensus path stays lightweight and the existing CI matrix stays green.
- Two funding paths must be kept in lockstep; the shared `TxRequest` →
  canonical-output helpers are the enforcement point, plus a cross-pool test
  asserting identical OP_RETURN + carrier bytes for a fixed logical transaction.
- The wallet store (SQLite) is a separate artifact from the consensus state DB
  (RocksDB) and is never read by the indexer.
