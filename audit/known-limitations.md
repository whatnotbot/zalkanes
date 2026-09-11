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

## Cross-platform determinism

- Green in four local configurations: aarch64 debug/release (native Apple
  Silicon) and x86_64 debug/release (Rosetta 2 emulation, labeled as
  such); CI covers native Linux x86_64 debug/release on every push.
  Native ARM64-Linux and non-emulated x86_64-macOS runs remain open.

## Stress run

- The 100-deploy / 10,000-successful-call / 1,000-intentional-failure
  target was executed in-process through the REAL block processor with two
  isolated state databases and per-block root equality
  (`crates/zalkanes-testkit/tests/two_node_stress.rs`), and the live
  two-STACK form resynced the full public-testnet chain on an independent
  node with byte-identical roots (`live-acceptance-evidence.md`). An
  on-chain public-testnet stress at those counts remains future work.

## Mainnet

- `MAINNET_ACTIVATION_HEIGHT = None` throughout; no mainnet canary exists.
