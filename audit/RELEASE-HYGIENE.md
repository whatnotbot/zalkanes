# Release / branch hygiene

## Audit candidate tag

`audit-candidate-v0-rc1` → `79942f5068d91c96529f9b0c6f01bf530e4ee62d`

This tag is **immutable**. If audit findings require changes, a new
candidate (`audit-candidate-v0-rc2`) is created; rc1 is never moved or
rewritten, and published history is never rewritten.

## Branch protection — APPLIED (owner-approved)

`main` is protected. Verified settings:

| setting | value |
|---|---|
| pull request required | yes |
| required approving reviews | 1 |
| dismiss stale reviews | yes |
| required conversation resolution | yes |
| require branch up to date before merge | yes (`strict`) |
| required linear history | yes |
| force pushes | **disabled** |
| branch deletion | **disabled** |
| enforced for administrators | yes |

Required status checks (exact context names, queried from the check-runs
API rather than guessed):

- `fmt + clippy + test`
- `shielded wallet (clippy + test)`
- `cargo deny`
- `native ARM64 Linux (debug + release)`
- `live zebrad regtest (pinned v6.3.0)`

> Operational note: with `enforce_admins` on and one required approval, the
> repository owner cannot merge their own pull request unaided. That is the
> intended governance posture. An administrator can temporarily relax it via
> the protection API if a solo merge is genuinely required.

## Tag protection — APPLIED

Repository ruleset **`release-and-audit-tags`** (id `23013829`), target
`tag`, enforcement `active`, **no bypass actors**, covering:

- `refs/tags/audit-candidate-*`
- `refs/tags/v*`

Rules: `deletion`, `update`, `non_fast_forward` — so protected tags cannot
be deleted, moved, or force-updated.

Creating the ruleset did not touch any existing ref;
`audit-candidate-v0-rc1` still resolves to tag object
`48f0b7edb4b0fe5af1d4e4d3a91bbe496adf2dc7` → commit
`79942f5068d91c96529f9b0c6f01bf530e4ee62d`.

## Release integrity: rc1 is NOT cryptographically signed

`audit-candidate-v0-rc1` is an annotated but **unsigned** tag. GitHub
reports:

```
verification.verified = false
verification.reason   = "unsigned"
```

This is recorded rather than corrected: **rc1 is not rewritten to sign it**,
because re-tagging would destroy the immutability the candidate depends on.

Integrity of this candidate therefore rests on:

- the immutable tag ruleset above (no delete/move/force-update),
- the recorded commit SHA and tree SHA,
- the `Cargo.lock` digest and dependency pins in `CANDIDATE.txt`, and
- **reproducible builds** — anyone can rebuild at this commit and obtain the
  published artifact digests byte-for-byte.

**Requirement for the eventual post-audit final release tag:** it MUST be
cryptographically signed (GPG/SSH, `verification.verified = true`) or carry
an equivalent GitHub release attestation / build-provenance record. Signing
is a release-engineering gate for the final tag, not a retrofit for rc1.

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
