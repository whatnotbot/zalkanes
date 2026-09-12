# Release-candidate lifecycle

This document exists because of one verified property of the protocol manifest,
which makes "audited" a claim about an **exact candidate**, never about the
project.

## The invariant

`protocol/v0.toml` contains **both** activation constants:

```toml
[network]
mainnet_activation_height = "None"
testnet_activation_height = 4338100
regtest_activation_height = 1
```

and the protocol manifest hash is **SHA-256 over the exact bytes of that file**
(stated in the file's own header: *"any change to a consensus constant changes
the hash"*). Verified at the audit candidate:

```
$ git show 79942f50:protocol/v0.toml | shasum -a 256
57178628cebadad21da5e5c6495a7d646a55c0299609ea8939737744ab0b8752
```

Two consequences follow, and neither is avoidable:

**A. A fresh testnet activation changes the manifest.** Setting
`testnet_activation_height` to a new value changes the file bytes, so the
manifest hash changes, so the candidate identity changes. RC1 → RC2.

**B. A future mainnet activation height ALSO changes the manifest.** Setting
`mainnet_activation_height` to anything other than `"None"` changes the same
file. The audited testnet candidate → a final-mainnet candidate with a
different manifest hash.

Therefore **the artifact an auditor reviews is never byte-identical to the
artifact that activates mainnet**, unless the mainnet height was already set
when they reviewed it — which it must not be.

## Required lifecycle

```
  RC1  (audit-candidate-v0-rc1, 79942f50…, manifest 57178628…)
      |
      |  close stale-note investigation                        [DONE]
      |  merge readiness work                                   [needs review]
      v
  RC2  new SIGNED testnet candidate, new manifest hash
      |
      |  fresh frozen-RC public-testnet activation              [owner approval]
      |  acceptance evidence
      v
  EXTERNAL AUDIT of the EXACT RC2 identity
      |
      |  fixes, if any -> new RC, rerun affected evidence
      v
  choose a future MAINNET activation height
      |
      v
  FINAL MAINNET CANDIDATE  (manifest hash differs from the audited one)
      |
      |  auditor re-confirmation or written delta sign-off on THIS candidate
      |  full CI + reproducible builds + regenerated SBOMs
      |  deployment to nodes BEFORE the activation height
      v
  activation
```

## The rule this forces

> **An audit of RC2 does not audit any later candidate whose mainnet
> activation constant differs.**

No informal equivalence ("it's only one constant", "nothing else changed") is
acceptable. Mainnet release requires **one** of:

**A. Direct sign-off** — the auditor reviews and signs off on the exact final
mainnet candidate: its commit, its tag, its manifest hash.

**B. Written delta confirmation** — the auditor confirms, in writing, that:
1. the only consensus difference from the audited candidate is
   `mainnet_activation_height`;
2. they reviewed that exact diff; and
3. the exact final commit SHA and final manifest hash are named in their
   confirmation.

This is enforced mechanically. `scripts/mainnet-gate.sh` compares the audited
candidate's manifest hash against the final candidate's. If they differ — which
they always will, once a mainnet height is set — the
`auditor_final_candidate_signoff` gate must be satisfied, and its recorded
`final_commit` and `final_manifest_sha256` must match the candidate actually
being released. A sign-off naming a different commit does not count.

## Every candidate's required identity

Any RC, at every step above, must record:

| item | why |
|---|---|
| signed annotated tag | provenance; RC1's unsigned tag must not be repeated |
| tag object SHA | the tag is the release identity, not the branch |
| commit SHA | what was built |
| tree SHA | what the commit's content actually is |
| protocol manifest SHA-256 | the consensus identity |
| `Cargo.lock` SHA-256 | the dependency identity |
| CI run IDs + conclusions | proof the matrix was green *on that commit* |
| reproducible artifact digests + sizes | what operators install |
| SBOM digests | supply chain |

**Never recreate a tag to fix metadata.** A tag that is wrong is superseded by
the next monotonically increasing RC identifier; it is never moved.

## Current position

| candidate | status |
|---|---|
| RC1 `audit-candidate-v0-rc1` `79942f50…` | immutable, unsigned, manifest `57178628…`, `mainnet_activation_height = "None"` |
| RC2 | **not created.** Requires the readiness work to be merged first. |
| final mainnet candidate | **not created.** Requires an external audit first. |
