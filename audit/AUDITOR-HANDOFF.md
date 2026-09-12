# Auditor handoff — Zalkanes protocol v0

Everything below is scoped to EXACTLY one immutable commit.

## Candidate identity

| field | value |
|---|---|
| repository | `whatnotbot/zalkanes` |
| audit tag | `audit-candidate-v0-rc1` |
| tag object | `48f0b7edb4b0fe5af1d4e4d3a91bbe496adf2dc7` (annotated, **unsigned**) |
| **commit** | **`79942f5068d91c96529f9b0c6f01bf530e4ee62d`** |
| tree | `df1dfdc43a6524d94c7ea8e35846fd78c436d7f4` |
| **protocol manifest hash** | **`57178628cebadad21da5e5c6495a7d646a55c0299609ea8939737744ab0b8752`** |
| `Cargo.lock` sha256 | `a8e87a4c852a876390e583e1914931aa396b32b3865ceb336bdcd998aa376199` |
| `MAINNET_ACTIVATION_HEIGHT` | `None` |
| testnet activation | `4_338_100` (see caveat below) |

Verify independently:

```
git fetch --tags
git rev-list -n1 audit-candidate-v0-rc1     # 79942f50…
git rev-parse audit-candidate-v0-rc1^{tree} # df1dfdc4…
git show audit-candidate-v0-rc1:protocol/v0.toml | sha256sum
git show audit-candidate-v0-rc1:Cargo.lock  | sha256sum
```

The tag is **unsigned** (`verification.reason = "unsigned"`). It was NOT
rewritten to add a signature, because re-tagging would destroy the
immutability the candidate depends on. Integrity rests on the immutable tag
ruleset, the recorded SHAs/digests, and reproducible builds. The eventual
post-audit **final release tag must be signed** or carry equivalent
attestation.

## Toolchain and pins

Rust `1.88.0` (CI: `rustc 1.88.0 (6b00bc388 2025-06-23)`) · wasmi **`=2.0.0`**
(any change is a protocol change) · `zcash_primitives 0.30.1` ·
`orchard 0.15.5` · `pczt 0.9.3` · `zcash_client_sqlite 0.22.0` ·
`zcash_client_backend 0.24.0` · `zcash_protocol 0.10.6` · `rocksdb 0.22.0` ·
`age 0.12.1` · base node **Zebra 6.3.0**.

## CI at this exact commit — all green

| workflow | run id | result |
|---|---|---|
| CI (fmt+clippy+test, shielded wallet, cargo-deny, wasm32 contracts) | `34665254215` | success |
| arm64-determinism (**native** aarch64 Linux, debug + release) | `34665254292` | success |
| release-engineering (reproducible builds ×2 targets + **real disk-full**) | `34665254346` | success |
| live-zebra-regtest (real pinned zebrad v6.3.0) | `34665254285` | success |

## Reproducible artifacts (run `34665254346`)

| target | file | size | sha256 |
|---|---|---|---|
| `x86_64-unknown-linux-gnu` | `zalkanes-x86_64-unknown-linux-gnu` | 32,526,488 | `8a155abe1967652c55dbfeb20d3996bef45d5847105f2108e29cf4e0427d254f` |
| `aarch64-unknown-linux-gnu` | `zalkanes-aarch64-unknown-linux-gnu` | 30,186,280 | `12b1977c01e0603fae3795d380e684d575370aad45e0337ae8883a650fcdda77` |

Two independent clean builds per target were byte-identical (the job fails
otherwise). Each artifact ships `SHA256SUMS`, `MANIFEST.txt` (source commit,
target triple, exact rustc/cargo, `SOURCE_DATE_EPOCH`, protocol manifest
hash), and **14 CycloneDX SBOMs** (`*.cdx.json`, one per workspace crate).
**No container image was built for this candidate.**

## Where to look

| topic | document |
|---|---|
| scope | `SCOPE.md` |
| architecture | `ARCHITECTURE.md` |
| wire format / consensus rules | `PROTOCOL.md` |
| threat model | `THREAT-MODEL.md` |
| consensus-critical files | `CONSENSUS-CRITICAL-FILES.md` |
| state-root spec + vectors + determinism | `STATE-ROOT.md` |
| reorg model | `REORG.md` |
| wallet boundary, custody, type-state | `WALLET-PRIVACY.md` |
| fuzz campaign and findings | `FUZZING.md`, `fuzz/README.md` |
| testing inventory | `TESTING.md` |
| dependency pins | `DEPENDENCIES.md` |
| trust assumptions | `ASSUMPTIONS.md` |
| build reproducibility | `BUILD-REPRODUCIBILITY.md` |
| live testnet evidence | `LIVE-ACCEPTANCE-EVIDENCE.md` |
| testnet activation analysis | `TESTNET-ACTIVATION-AUDIT.md` |
| live regtest classification | `REGTEST-CLASSIFICATION.md` |
| open items | `KNOWN-LIMITATIONS.md` |

## Evidence summary

- **Fuzzing:** 5 targets, 412M+ executions. Two findings, both fixed with
  committed regression seeds and permanent unit regressions:
  (1) rollback round-trip corruption via deploy-replacement and the
  zero-length-key sentinel; (2) a wasmi translator **panic** on a malformed
  module (node-halt class), now contained into deterministic rejection.
  **Unresolved: 0.** Fuzzing does not run in CI; the regressions do.
- **Determinism:** native aarch64-darwin, native x86_64 Linux (CI), native
  aarch64 Linux (CI), plus x86_64-darwin under Rosetta (labeled emulation).
  101 committed execution vectors + 100 hashing vectors + 5 protocol vectors,
  with an independent Python hashing implementation.
- **Two-node equality:** an independent empty-database node resynced the full
  post-activation public-testnet chain and reproduced the other node's root
  byte-for-byte at height 4,339,534. Plus 100 deployments / 10,000 successful
  calls / 1,000 intentional failures across two isolated stores with
  per-block root equality.
- **Crash/corruption:** SIGKILL during commit and reindex, interrupted
  rollback, read-only files, truncated `CURRENT`, corrupt `MANIFEST`, and a
  **real disk-full** on a 48 MiB ext4 loopback (163 commits, loud failure,
  then a clean refusal to reopen).
- **Live testnet:** shielded CALLs (2→3→4), transparent/shielded ZALK payload
  byte equality, and a shielded-funded PREPARE → transparent carrier DEPLOY
  producing a fresh contract executed 0→1→2.

## Priority areas for review

1. consensus parser / carrier parsing
2. WASM determinism and fuel metering
3. state-root construction
4. rollback / undo-journal correctness
5. database atomicity
6. transaction construction
7. `VerifiedPczt` / `VerifiedTransaction` type-state boundaries
8. fee and value-balance verification
9. shielded change / spend / nullifier / anchor binding
10. journal and note-lock recovery
11. `BroadcastUnknown` handling
12. wallet key custody
13. reorg handling
14. activation-boundary semantics

## OPEN HARD GATES — please read before concluding

1. **Live note-level competing-branch reorg is OPEN.** A shielded note
   received on a removed branch disappearing, and a spend rolling back, have
   **not** been observed against a live node. Inducing this requires
   constructing and submitting a competing chain; that machinery does not
   exist in this repository. The deterministic wallet reorg matrix is
   supporting evidence, **not a substitute**. See `REGTEST-CLASSIFICATION.md`.
2. **Activation-boundary live reorg is OPEN** (rollback below the
   pre-activation fast-forward point).
3. **Fresh post-freeze frozen-RC public-testnet activation is OPEN.** The
   existing testnet activation height predates the final protocol freeze, so
   the current live evidence is **frozen-CODE acceptance**, NOT a fresh
   frozen-RC activation. See `TESTNET-ACTIVATION-AUDIT.md`.
4. **External security audit is OPEN** — this document is its starting point.

Any Critical or High finding is a hard blocker. Fixes produce
`audit-candidate-v0-rc2` at a new commit; **rc1 is never rewritten.**

## Mainnet status

`MAINNET_ACTIVATION_HEIGHT = None`. No mainnet ZEC has been spent, no canary
has been run, and no activation has been announced or scheduled.
