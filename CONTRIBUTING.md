# Contributing to Zalkanes

## Status

Zalkanes is pre-audit, pre-testnet software. External contributions are welcome
but please read this document before opening a PR.

## Ground rules

1. **No consensus changes without an ADR.** Any change to the parser, WASM
   execution profile, fuel schedule, state root algorithm, or host ABI requires
   a new or updated ADR in `docs/adr/` and new test vectors.

2. **Pin all dependencies.** Do not add `git = "...", branch = "main"` for any
   released protocol code. Use exact crate versions and commit to `Cargo.lock`.

3. **Tests are mandatory.** Every PR needs tests. Consensus-affecting changes
   need deterministic fixture vectors.

4. **`#![forbid(unsafe_code)]`** is the target for all consensus crates. Any
   unavoidable unsafe requires a security justification comment, an isolated
   module, and tests.

5. **No out-of-scope features.** See spec §51 for the explicit exclusion list.
   Do not open PRs for bridges, DEXes, governance, or sequencers.

## PR sequence

Follow the recommended PR sequence from the spec:
PR-001 scaffolding → PR-002 chain source → PR-003 carrier spike → …

Do not submit one enormous implementation PR.

## CI

Every PR must pass:

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo test --release --workspace
cargo deny check
```

## Code style

- Standard `rustfmt` formatting (enforced by CI)
- `clippy` warnings are errors
- Public APIs need doc comments
- Consensus constants must be named and documented in `zalkanes-core`

## License

By contributing you agree that your contributions are licensed under the MIT
License (same as the project).
