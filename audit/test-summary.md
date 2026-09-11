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

## Not yet run (release-blocking)

- `cargo-fuzz` campaign (see `fuzz-summary.md`).
- ARM64 determinism matrix.
- Two-node root-agreement soak.
- 100-deploy / 10,000-call stress run.
- Activation-boundary reorg test.
- State-DB corruption (kill -9 / disk-full / corrupt journal) tests.
