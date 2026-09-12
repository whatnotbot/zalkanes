# Known Limitations

These are limitations that are **known and tracked**, not silent gaps. Auditors
should confirm whether any of these change the security conclusion.

## Declared-but-not-fully-enforced consensus constants

| Constant | Status |
|----------|--------|
| `MAX_FUNCTIONS`, `MAX_GLOBALS`, `MAX_TABLE_ELEMENTS`, `MAX_LINEAR_MEMORY_PAGES`, `MAX_IMPORTS`, `MAX_EXPORTS` | **Enforced** via `wasmparser` section counting in `validate_module`. |
| `MAX_FUEL_PER_ZCASH_TX`, `MAX_FUEL_PER_ZCASH_BLOCK` | **Enforced** in the indexer (per-tx and per-block fuel budgets). |
| `MAX_STORAGE_WRITES_PER_CALL` | Enforced (distinct overlay keys). |
| `MAX_ZALK_MESSAGES_PER_TX`, `MAX_ZALK_MESSAGES_PER_BLOCK`, `MAX_CARRIER_BYTES_PER_BLOCK`, `MAX_DEPLOY_BYTES_PER_BLOCK` | **Enforced** in the indexer. |

## Structural limitations

- The indexer fetches one block per RPC call (plus the pre-activation
  fast-forward). This is correct but not maximally efficient.
- Rollback below the fast-forward point resets to empty state and re-runs the
  fast-forward (correct, but not yet covered by a dedicated activation-boundary
  reorg test).

## Fuzzing

- Executed 2026-09-12 with two findings, both fixed with regression seeds
  and unit regressions; zero unresolved. See `fuzz-summary.md` and
  `fuzz/README.md`. Continuous (long-horizon) fuzzing remains future work.

## Disk exhaustion

- **Covered.** A CI job mounts a 48 MiB ext4 loopback filesystem and runs
  RocksDB against it until it physically fills. Observed: 163 successful
  commits, then `rocksdb write batch failed` (loud), and the store then
  **refused to reopen** on the full filesystem — an acceptable
  deterministic outcome (never a silent partial commit). See
  `crates/zalkanes-state/tests/disk_full.rs` and the `release-engineering`
  workflow.

## Cross-platform determinism

- Green in four local configurations: aarch64 debug/release (native Apple
  Silicon) and x86_64 debug/release (Rosetta 2 emulation, labeled as
  such); CI covers native Linux x86_64 debug/release on every push.
  **Native ARM64 Linux is now covered** by the `arm64-determinism` workflow
  (GitHub-hosted `ubuntu-24.04-arm`), debug and release. Only non-emulated
  x86_64-macOS remains unrun, and it is not a release blocker given native
  x86_64 Linux CI coverage.

## Stress run

- The 100-deploy / 10,000-successful-call / 1,000-intentional-failure
  target was executed in-process through the REAL block processor with two
  isolated state databases and per-block root equality
  (`crates/zalkanes-testkit/tests/two_node_stress.rs`), and the live
  two-STACK form resynced the full public-testnet chain on an independent
  node with byte-identical roots (`live-acceptance-evidence.md`). An
  on-chain public-testnet stress at those counts remains future work.

## Reproducible builds

- **Enforced and verified.** Two independent clean builds per target are
  byte-identical (x86_64 + aarch64 Linux); see `BUILD-REPRODUCIBILITY.md`
  for the recorded digests.

## Live regtest

- The `live-zebra-regtest` workflow is **GREEN** against a real pinned
  zebrad v6.3.0: wallet create against a real treestate, restore at an
  explicit birthday, scanning real blocks, scanned identity == canonical
  tip, advancing-tip tracking, restart identity, LOCKED refusal of a
  shielded spend, unlock/lock, and dry-run hygiene.
- **Note-level branch reorg remains uncovered** (a note on a removed
  branch disappearing, a spend rolling back): inducing it requires
  constructing and submitting a COMPETING chain, machinery this repo does
  not have. Classification E. **Mainnet blocker.** Full evidence and the
  four bugs this workflow found: `REGTEST-CLASSIFICATION.md`.
- A per-process request ceiling in the CI environment stalls a ~100-block
  scan at a fixed point (node verified healthy via identical curl
  requests); the workflow scopes its scan inside that ceiling. Documented
  in `REGTEST-CLASSIFICATION.md`.
- `z_gettreestate` at **genesis** (height 0) returns a body the wallet
  deserializer rejects. Reachable only with a birthday of 0, i.e. a chain
  shorter than 100 blocks. Tracked; not a release blocker.

## Testnet activation

- The current public-testnet activation is **not** a fresh frozen-RC
  activation; see `TESTNET-ACTIVATION-AUDIT.md` for the chronology, the
  verdict, and the (unexecuted) fresh-activation plan. Requires approval.

## Mainnet

- `MAINNET_ACTIVATION_HEIGHT = None` throughout; no mainnet canary exists.
