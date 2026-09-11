# Known Limitations

These are limitations that are **known and tracked**, not silent gaps. Auditors
should confirm whether any of these change the security conclusion.

## Declared-but-not-fully-enforced consensus constants

| Constant | Status |
|----------|--------|
| `MAX_FUNCTIONS`, `MAX_GLOBALS`, `MAX_TABLE_ELEMENTS`, `MAX_LINEAR_MEMORY_PAGES`, `MAX_IMPORTS`, `MAX_EXPORTS` | **Enforced** via `wasmparser` section counting in `validate_module`. |
| `MAX_FUEL_PER_ZCASH_TX` | **Not aggregated.** `MAX_FUEL_PER_CALL` is enforced per call; per-transaction fuel is not yet summed. |
| `MAX_FUEL_PER_ZCASH_BLOCK` | **Not aggregated.** Same as above at block scope. |
| `MAX_STORAGE_WRITES_PER_CALL` | Enforced (distinct overlay keys). |

The per-tx/per-block fuel aggregation is a **release-blocking** item tracked
under the block-DoS workstream; it is not yet complete.

## Structural limitations

- The indexer fetches one block per RPC call (plus the pre-activation
  fast-forward). This is correct but not maximally efficient.
- Rollback below the fast-forward point resets to empty state and re-runs the
  fast-forward (correct, but not yet covered by a dedicated activation-boundary
  reorg test).

## Fuzzing

- `cargo-fuzz` targets are declared in the milestone plan but a long-running
  fuzz campaign has **not yet been executed**. See `fuzz-summary.md`.

## Cross-platform determinism

- Determinism on Linux x86_64 (debug + release) is covered by CI. ARM64
  determinism is **not yet verified** in CI. See `test-summary.md`.

## Stress run

- A 100-deploy / 10,000-call public-testnet stress run is **not yet executed**.
  The functional deploy/call/restart/reindex acceptance evidence is in
  `testnet-evidence.md`.

## Mainnet

- `MAINNET_ACTIVATION_HEIGHT = None` throughout; no mainnet canary exists.
