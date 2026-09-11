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

## Not yet run (release-blocking)

- Activation-boundary reorg test (live regtest environment required).
- Real disk-full behavior (no loop-device support in the dev environment).
- Native ARM64-Linux / non-emulated x86_64-macOS determinism runs.
- Live regtest note-level reorg evidence (removed-branch note/spend).
- External security audit.
