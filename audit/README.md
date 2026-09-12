# Zalkanes Protocol v0 — Audit Package

Single entry point for an external security/consensus auditor.

## Candidate under audit

| item | value |
|---|---|
| **Git commit** | see `CANDIDATE.txt` (written at freeze; verify with `git rev-parse HEAD`) |
| **Protocol manifest hash** | `06e3df62e5e98a3c276b583e038cbea8d05934e9d2a8e7c00299fec3140bf4bb` (SHA-256 of `protocol/v0.toml`) |
| **Protocol status** | v0, `FROZEN-RC` |
| **Mainnet activation** | `MAINNET_ACTIVATION_HEIGHT = None` — contract execution is disabled on mainnet by construction |
| **Testnet activation** | height 4,338,100 |

Verify the manifest hash yourself:

```
sha256sum protocol/v0.toml
curl -s -X POST $ZALKANES_URL -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"zalkanes_getInfo","params":[]}'
```

Any change to a consensus constant changes this hash.

## Index

| File | Purpose |
|------|---------|
| `SCOPE.md` | What is and is not in scope |
| `ARCHITECTURE.md` | System architecture |
| `PROTOCOL.md` | Wire format + consensus rules (canonical: `docs/protocol-v0.md`) |
| `THREAT-MODEL.md` | Threat model (canonical: `docs/threat-model/*`) |
| `CONSENSUS-CRITICAL-FILES.md` | Every source file that can change consensus |
| `DEPENDENCIES.md` | Pinned dependencies (canonical: `docs/upstream-lock.md`) |
| `ASSUMPTIONS.md` | Explicit trust assumptions |
| `KNOWN-LIMITATIONS.md` | Known limitations and open items |
| `TESTING.md` | Test coverage and what has/has not been run |
| `FUZZING.md` | Fuzz campaign, findings, and fixes |
| `STATE-ROOT.md` | Frozen root algorithm, vectors, determinism evidence |
| `REORG.md` | Indexer + wallet reorg model and coverage |
| `WALLET-PRIVACY.md` | Privacy boundary, key custody, authorization type-state |
| `BUILD-REPRODUCIBILITY.md` | Pinned toolchain, two-clean-build gate, artifacts, SBOM |
| `TESTNET-EVIDENCE.md` | Public-testnet acceptance history |
| `LIVE-ACCEPTANCE-EVIDENCE.md` | Sanitized live transaction records (no secrets) |

Operator-facing documentation: `docs/operator-guide.md`.

## What an auditor should know up front

1. **Zalkanes does not re-verify Zcash consensus.** It trusts the operator's
   own Zebra node (pinned 6.3.0). A compromised or forked Zebra produces a
   divergent index. See `ASSUMPTIONS.md`.
2. **Contract interaction is public.** The ZALK message, deployed code, and
   all contract state are cleartext on chain. Shielded wallet funding hides
   only the source of funds. See `WALLET-PRIVACY.md`.
3. **The state root is a flat sorted BLAKE2b hash, not a Merkle tree.** It
   commits to contract code and storage only. See `STATE-ROOT.md`.
4. **There is no cross-contract call in v0** (no `contract_call` host
   function). `MAX_CALL_DEPTH` exists but is unreachable.
5. **Two separate databases with no cross-store atomicity**: RocksDB
   consensus state and the wallet SQLite store. Recovery never assumes they
   moved together.
6. **Known consensus divergences from older docs are documented, not hidden.**
   Where an ADR and the implementation disagree, the audit document states
   which is authoritative.

## Evidence summary

| area | status |
|---|---|
| Fuzzing | executed; 2 findings, both fixed with regression seeds; 0 unresolved (`FUZZING.md`) |
| State-root vectors | 101 execution + 100 hashing + 5 protocol vectors, all consumed by tests |
| Determinism | native aarch64-darwin, native x86_64-linux (CI), native aarch64-linux (CI), plus x86_64-darwin under Rosetta (labeled) |
| Two-node equality | live public-testnet resync byte-identical at height 4,339,534; plus 100 deploys / 10,000 calls / 1,000 failures in isolated stores |
| Crash/corruption | SIGKILL, interrupted rollback, read-only, truncated CURRENT, corrupt MANIFEST, real disk-full on an ext4 loopback in CI |
| Live wallet acceptance | shielded CALLs, payload equality, shielded-funded deploy (`LIVE-ACCEPTANCE-EVIDENCE.md`) |
| Reproducible builds | two-clean-build gate enforced in CI (`BUILD-REPRODUCIBILITY.md`) |
| External audit | **NOT DONE — hard mainnet gate** |

## Status

This package is **not** production-final. External audit is a hard gate
before any mainnet activation, which remains `None`. Open items are
enumerated in `KNOWN-LIMITATIONS.md`; none are silently marked complete.
