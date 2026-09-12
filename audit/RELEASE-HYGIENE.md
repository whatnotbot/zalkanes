# Release / branch hygiene

## Audit candidate tag

`audit-candidate-v0-rc1` → `79942f5068d91c96529f9b0c6f01bf530e4ee62d`

This tag is **immutable**. If audit findings require changes, a new
candidate (`audit-candidate-v0-rc2`) is created; rc1 is never moved or
rewritten, and published history is never rewritten.

## Branch protection — REQUIRED ADMINISTRATIVE STEP (not applied)

`main` is currently **unprotected** (verified: the branch-protection API
returns 404 "Branch not protected"). Protection changes how everyone pushes
to this repository, so it is a governance decision for the repository owner
rather than something to enable unilaterally. It has therefore been
documented, not applied.

To require green CI before anything reaches `main`:

```
gh api -X PUT repos/<owner>/zalkanes/branches/main/protection \
  -F required_status_checks.strict=true \
  -f 'required_status_checks.contexts[]=fmt + clippy + test' \
  -f 'required_status_checks.contexts[]=shielded wallet (clippy + test)' \
  -f 'required_status_checks.contexts[]=native ARM64 Linux (debug + release)' \
  -f 'required_status_checks.contexts[]=live zebrad regtest (pinned v6.3.0)' \
  -F enforce_admins=true \
  -F required_pull_request_reviews.required_approving_review_count=1 \
  -F restrictions=null
```

Recommended additionally:

- **Tag protection** for `audit-candidate-*` and `v*` so release tags cannot
  be moved or deleted.
- **Require linear history** and disallow force-pushes to `main`.

Until these are applied, the immutability of the audit candidate rests on
convention plus the recorded commit SHA and artifact digests in
`CANDIDATE.txt` — an auditor can always verify independently with
`git rev-list -n1 audit-candidate-v0-rc1` and by re-running the
reproducible-build workflow at that commit.

## CI coverage at the candidate

| workflow | covers |
|---|---|
| `CI` | fmt, clippy `-D warnings`, `cargo test --workspace` in **debug and release** (so the release-gated two-node scale stress and all committed state-root/execution vectors run), shielded-wallet clippy + tests, `cargo-deny` |
| `arm64-determinism` | native aarch64 Linux, debug + release, incl. execution vectors |
| `release-engineering` | two independent clean builds per target compared byte-for-byte, SBOM, checksums, manifest; real disk-full on an ext4 loopback |
| `live-zebra-regtest` | real pinned zebrad v6.3.0 baseline |

**Fuzzing is not run in CI** (campaigns are bounded and run locally; see
`FUZZING.md`). Both fuzz findings have permanent **unit-test regressions**
that DO run in CI — the wasmi translator-panic fixture in
`crates/zalkanes-runtime/tests/adversarial_wasm.rs` and the rollback
round-trip coverage in `crates/zalkanes-state`. Minimized crash inputs are
committed under `fuzz/seeds/`.
