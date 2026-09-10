# Zalkanes Protocol v0 — Audit Package (RC)

This directory is the single entry point for an external security/consensus
auditor. It freezes the exact commit under review and points to the normative
specification, consensus-critical source, dependency pins, threat model, and
evidence.

## Commit under audit

- **Git commit:** `e022bac4add584a956f7e437d0e9fa821c6a190e`
- **Protocol manifest hash:** `PROTOCOL_V0_MANIFEST_HASH` (SHA-256 over
  `protocol/v0.toml`); see `zalkanes_getInfo.protocol_manifest_hash`.

> The manifest hash above is the hash of the manifest at the audited commit.
> Any change to a consensus constant changes it.

## Index

| File | Purpose |
|------|---------|
| `scope.md` | What is and is not in scope |
| `architecture.md` | System architecture (canonical: `docs/architecture.md`) |
| `protocol-v0-frozen.md` | Wire format + consensus rules (canonical: `docs/protocol-v0.md`) |
| `threat-model.md` | Threat model (canonical: `docs/threat-model/*`) |
| `consensus-critical-files.md` | Every source file that can change consensus |
| `dependency-lock.md` | Pinned dependencies (canonical: `docs/upstream-lock.md`) |
| `known-assumptions.md` | Explicit assumptions |
| `known-limitations.md` | Known limitations and un-enforced constants |
| `test-summary.md` | Test coverage summary |
| `fuzz-summary.md` | Fuzzing status |
| `testnet-evidence.md` | Public-testnet acceptance evidence |

## Status

Protocol v0 is **DRAFT** (pre-freeze). This package tracks the freeze process;
it will be marked **FROZEN-RC** when the release candidate is tagged.
