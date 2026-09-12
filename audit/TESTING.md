# Test Summary

## Automated suites (all run in CI via `cargo test --workspace`)

| Suite | Coverage |
|-------|----------|
| `zalkanes-core` | ContractId network-distinctness, consensus-params boundaries, manifest hash determinism |
| `zalkanes-protocol` | Round-trips; adversarial parser (`tests/adversarial.rs`): empty/truncated header, wrong magic, unknown version/type, trailing bytes, code/input/chunk limits, CALL_CARRIER |
| `zalkanes-carrier` | Adversarial reconstruction (`tests/adversarial.rs`): missing/duplicate/out-of-range/reordered chunks, wrong length/hash, over-max length |
| `zalkanes-state` | Memory + RocksDB root/rollback suites, persistence across reopen |
| `zalkanes-runtime` | Enforcement (`tests/enforcement.rs`): memory pages, table elements, import/export counts, code size, malformed wasm — each at limit/limit+1 |
| `zalkanes-tx` | Redeem-script shape, chunk-size policy, carrier input count |
| `zalkanes-indexer` / `zalkanes-rpc` / `zalkanes-testkit` | Indexing + RPC + in-memory E2E |

## CI gates

`fmt`, `clippy -D warnings`, `tests (dev)`, `tests (release)`, `contract wasm32
builds`, `cargo-deny` — all must be green. See `.github/workflows/ci.yml`.

## Live-chain evidence

- Regtest (Milestone 1): counter deploy + 2 calls + `get()==2`, accepted.
- Public testnet (Milestone 2, V4-era): full evidence in `docs/release/testnet-activation.md`.
- **V5/ZIP-244 (Milestone 3 freeze):**
  - Public testnet (Nu6.3): V5 deploy `6a49cc08…` (version 5, vgid `26a7270a`),
    calls `2a95a672…` / `499306be…`, `get()==2`, contract `4e70415c…`.
  - Regtest (Nu6.3, reconfigured): carrier relay accepted + reconstructed
    (PREPARE `d0c95255…`, DEPLOY `51b20b8c…`, 1400-byte chunk round-trip).

## Completed since the freeze (2026-09-12 hardening phase)

- `cargo-fuzz` campaign: executed, 412M+ executions, two findings fixed,
  zero unresolved (`fuzz-summary.md`, `fuzz/README.md`).
- Cross-arch determinism: aarch64 debug/release native + x86_64
  debug/release under Rosetta 2 (labeled emulation), plus CI's native
  Linux x86_64 — all green including the 101 committed execution vectors.
- Two-node root agreement: independent local node resynced the full
  public-testnet chain and reproduced the Railway node's root
  byte-for-byte at height 4,339,534 (`live-acceptance-evidence.md`).
- 100-deploy / 10,000-call / 1,000-failure stress: in-process through the
  real block processor with per-block root equality across two isolated
  DBs (`two_node_stress.rs`).
- State-DB crash/corruption suite: SIGKILL during commit/reindex,
  interrupted rollback, read-only refusal, truncated CURRENT, corrupted
  MANIFEST (`crates/zalkanes-state/tests/crash.rs`).
- WASM adversarial suite + frozen-profile enforcement
  (`crates/zalkanes-runtime/tests/adversarial_wasm.rs`).
- Wallet reorg matrix incl. same-height/shorter-branch detection fix
  (`crates/zalkanes-wallet/src/wallet_reorg_tests.rs`).

## Completed since the previous revision

- **Real disk-full**: CI mounts a 48 MiB ext4 loopback and fills it —
  163 commits, then a loud failure, then a clean refusal to reopen
  (`release-engineering` workflow, `tests/disk_full.rs`).
- **Native ARM64 Linux determinism**: `arm64-determinism` workflow on
  GitHub-hosted `ubuntu-24.04-arm`, debug + release, including the
  committed execution vectors.
- **Live zebrad regtest baseline**: `live-zebra-regtest` runs the pinned
  zebrad v6.3.0 and is GREEN — wallet create against a real treestate,
  restore at an explicit birthday, scanning real blocks, scanned identity
  == canonical tip, advancing-tip tracking, restart identity, LOCKED
  refusal of a shielded spend, unlock/lock, EPIPE handling, dry-run
  hygiene. See `REGTEST-CLASSIFICATION.md`.
- **Reproducible builds**: two clean builds byte-identical per target
  (`BUILD-REPRODUCIBILITY.md`).
- **CLI acceptance**: 12 tests against the real binary.
- **Key custody**: 16 tests (keystore + lifecycle).

## Not yet run

- **Live note-level branch reorg** (a note on a removed branch
  disappearing; a spend rolling back). Requires competing-chain
  construction/submission machinery that does not exist in this repo.
  Classification E. **Mainnet blocker.**
- **Activation-boundary reorg test** (rollback below the pre-activation
  fast-forward point). Mainnet blocker.
- **Fresh frozen-RC public testnet activation** — see
  `TESTNET-ACTIVATION-AUDIT.md`. Requires explicit approval. Mainnet
  blocker.
- **Genesis-height treestate** handling: `z_gettreestate` at height 0
  returns a body the wallet deserializer rejects. Only reachable with a
  birthday of 0 (a chain shorter than 100 blocks). Not a release blocker;
  tracked.
- **Non-emulated x86_64-macOS** determinism. Not a release blocker given
  native x86_64 Linux CI coverage.
- **External security audit** — hard mainnet gate.
