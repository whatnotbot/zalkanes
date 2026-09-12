# Mainnet release gate

Mainnet release is **impossible** until every gate below is satisfied. This
document is the human-readable form; `scripts/mainnet-gate.sh` is the
machine-checkable form and is the authority.

```
scripts/mainnet-gate.sh --artifacts <release-artifact-dir>
```

The script exits `0` only when every gate passes and exits `1` otherwise. It
**never sets a gate** — it only recomputes and reports.

## Current result

`MAINNET RELEASE BLOCKED`. `mainnet_activation_height = "None"` and no mainnet
ZEC has been spent. See "Status today" below.

## How the gates are proven

Gates come in two kinds, and the distinction is the whole point.

**Mechanical gates** are recomputed from the repository and from GitHub on
every run: the mainnet activation constant, the protocol manifest digest, the
`Cargo.lock` digest, the candidate tag identity, the tag signature, the working
tree, the required CI contexts on the candidate commit, artifact checksums and
SBOM presence. Nothing is taken on trust.

**Evidence-backed gates** live in `audit/gate-attestations.json`. Each one
names an evidence file and that file's SHA-256. A gate counts as satisfied only
when `satisfied: true` **and** the evidence file exists **and** its digest still
matches. Flipping a boolean without producing real evidence fails the gate, and
editing the evidence afterwards fails it too.

No gate is ever asserted from reasoning, expectation, or a passing unit test.

## The gates

| # | gate | kind | how it is proven |
|---|---|---|---|
| 1 | Repository state clean and required CI green | mechanical | `git status --porcelain` empty; every context in `main`'s branch protection reports `success` for the candidate commit |
| 2 | Immutable audited candidate identity recorded | mechanical | tag object and tag→commit both equal the recorded values |
| 3 | Live note-level reorg PASS | evidence | real competing-branch regtest run: a received note disappears, a spend rolls back, and both survive a restart |
| 4 | Activation-boundary live reorg PASS | evidence | real reorg across the Zalkanes activation height; persisted root == clean-reindex root == restart root |
| 5 | Fresh frozen-RC public-testnet activation PASS | evidence | `audit/TESTNET-ACTIVATION-RUNBOOK.md` executed end to end, with owner approval |
| 6 | External security audit complete | evidence | the auditor's final report |
| 7 | Zero unresolved Critical findings | evidence | disposition table naming every Critical finding and its fix |
| 8 | Zero unresolved High findings | evidence | as above, for High |
| 9 | Any fixes produced a NEW immutable audited RC | evidence | the new tag and commit, recorded and immutable |
| 10 | Final candidate CI green | mechanical | same check as #1, against the final candidate |
| 11 | Reproducible artifacts verified | mechanical | `shasum -a 256 -c SHA256SUMS` in the artifact directory |
| 12 | SBOM regenerated/verified | mechanical | CycloneDX documents present for every target |
| 13 | Protocol manifest hash frozen | mechanical | SHA-256 of `protocol/v0.toml` equals the recorded digest |
| 14 | `Cargo.lock` digest frozen | mechanical | SHA-256 of `Cargo.lock` equals the recorded digest |
| 15 | Final release tag signed or equivalent provenance | mechanical | `git verify-tag` succeeds |
| 16 | Two independent nodes reproduce the same public-testnet root | evidence | both nodes' `zalkanes_getInfo` at an identical height, node B built from an empty database |
| 17 | Mainnet activation height chosen only after all the above | mechanical | the script refuses this gate while any other gate is unsatisfied |
| 18 | Activation announcement/runbook reviewed | evidence | owner sign-off on the mainnet activation runbook |
| 19 | Mainnet canary explicitly owner-approved | evidence | recorded owner approval; never inferred |

## Status today

Eight gates pass. Six are mechanical: the mainnet activation height is `None`
in both the manifest and the source, the protocol manifest digest and
`Cargo.lock` digest match the frozen values, the candidate tag still resolves to
the recorded commit, the working tree is clean, and all five required CI
contexts are green on `79942f50…`.

Two are evidence-backed and newly closed, both proven against real software
rather than a deterministic matrix (`audit/LIVE-REORG-EVIDENCE.md`, CI run
34682825442): **live note-level reorg** and **activation-boundary live reorg**.
The note-level evidence carries one OPEN observation — a note row survives a
reorg that removes its block. The spendable balance is 0 and the production
spend planner refuses it (CI asserts that refusal), but the diagnostic counter
still reports it and no root cause has been established.

Everything else is blocked. Three blocks are worth calling out because they are
not merely "not done yet":

1. **The candidate tag is unsigned.** `audit-candidate-v0-rc1` was created
   annotated but unsigned. It is deliberately **not** being rewritten to add a
   signature — re-tagging would destroy the immutability the candidate depends
   on. The gate stays blocked, and the *final* release tag must be signed.
   See `audit/RELEASE-HYGIENE.md`.
2. **The fresh testnet activation creates a new candidate.** The testnet
   activation height is a constant inside `protocol/v0.toml`, and the manifest
   hash is SHA-256 over that file. Choosing a new height necessarily changes the
   manifest hash and therefore necessarily produces a new immutable RC. Gate 5
   cannot be closed without also touching gates 9, 13 and 14. See
   `audit/TESTNET-ACTIVATION-RUNBOOK.md` §0.
3. **No external audit has been commissioned**, so gates 6, 7 and 8 are not
   merely unsatisfied — they are unsatisfiable today. Gates 7 and 8 must never
   be set to `true` on the grounds that no findings exist; absence of an audit
   is not absence of findings.

## Rules for changing this file

- Never mark a gate satisfied without evidence that the script can re-verify.
- Never weaken a check to make the script pass.
- If a gate genuinely does not apply, remove it in a reviewed pull request with
  the reasoning recorded — do not silently set it to `true`.
