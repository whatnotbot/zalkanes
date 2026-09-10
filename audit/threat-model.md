# Threat Model

Canonical: `docs/threat-model/` (parser, wasm, state, data-availability,
chain-reorg).

Summary of the security model: a single locally-operated, consensus-validating
Zebra node is the trust anchor; Zalkanes itself must (a) never panic or mutate
state on malformed input, (b) execute contracts atomically and deterministically,
(c) derive the state root as a pure function of chain data, and (d) never
diverge from the canonical chain across reorgs. See `docs/threat-model/*` for
the per-component adversary models.
