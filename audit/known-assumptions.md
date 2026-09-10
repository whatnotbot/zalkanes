# Known Assumptions

1. **Own Zebra full node is the trust anchor.** Zalkanes trusts exactly one
   consensus-validating Zebra node for block data; it does not independently
   re-verify Zcash consensus (PoW, signatures, chainwork). A compromised or
   forked Zebra would feed a divergent Zalkanes index. (This is the documented
   security model — see `docs/architecture.md`.)

2. **The carrier redeem script is non-standard but consensus-valid.** The
   36-byte `<pubkey> OP_CHECKSIG OP_NOP` script is accepted by Zebra's
   standardness policy (1 sigop ≤ 15, "extra data left on stack is OK"). This is
   a policy property, not a consensus guarantee; a future policy change could
   reject the carrier and would require a protocol change. (See ADR-0003.)

3. **V5 transactions remain accepted.** Zalkanes signs V5 (ZIP-244) transparent
   transactions; `valid_in_branch` confirms V5 is accepted through Nu6.3. A
   future network upgrade that drops V5 would require a protocol change.

4. **OP_RETURN policy limit is 80 bytes.** The inline CALL size (38 bytes) is
   derived from the standard `MAX_OP_RETURN_RELAY = 80` policy. A policy change
   would not break parsing (parsers are length-driven) but could make larger
   inline messages relayable — not assumed.

5. **SHA-256 and BLAKE2b-256 are collision-resistant.** `code_hash`,
   `input_hash`, and all state-root hashes rely on these.

6. **Wasmi 2.0.0 is deterministic** across x86_64/ARM64 with the frozen config
   (eager compilation, fuel metering, no floats/SIMD/threads). This is the
   cross-platform determinism requirement under test.

7. **secp256k1 ECDSA signatures are deterministic-ish and verifiable.** The
   signer uses librustzcash's canonical V5 sighash; a wrong branch id makes the
   signature invalid under consensus.

8. **Pre-activation state is provably empty.** The fast-forward to
   `activation_height - 1` assumes no Zalkanes messages exist below activation;
   this is guaranteed by choosing an activation height above the pre-activation
   tip.
