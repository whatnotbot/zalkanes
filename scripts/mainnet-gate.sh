#!/usr/bin/env bash
# Mainnet release gate.
#
# Exits 0 ONLY when every gate is satisfied. Mechanical gates are recomputed
# here from the repository and from GitHub; evidence-backed gates are read
# from audit/gate-attestations.json and are accepted only when the named
# evidence file exists and still hashes to the recorded digest.
#
# This script never sets a gate. It only reports.
#
# Usage:
#   scripts/mainnet-gate.sh [--candidate <tag>] [--artifacts <dir>] [--no-network]
set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

ATTEST="audit/gate-attestations.json"
CANDIDATE=""
ARTIFACTS=""
NETWORK=1

while [ $# -gt 0 ]; do
  case "$1" in
    --candidate) CANDIDATE="$2"; shift 2 ;;
    --artifacts) ARTIFACTS="$2"; shift 2 ;;
    --no-network) NETWORK=0; shift ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done

PASS=0
FAIL=0

sha256() { shasum -a 256 "$1" 2>/dev/null | cut -d' ' -f1; }
jqa() { jq -r "$1" "$ATTEST" 2>/dev/null; }

report() { # state, name, detail
  # UNKNOWN is NOT a pass. It marks a gate that cannot even be evaluated yet
  # (e.g. "zero unresolved Critical findings" before any audit exists). It
  # counts as unsatisfied exactly like BLOCKED; only the label differs, so that
  # "not yet answerable" is never read as "answered yes".
  case "$1" in
    PASS)    printf '  [PASS]    %-46s %s\n' "$2" "$3"; PASS=$((PASS+1)) ;;
    UNKNOWN) printf '  [UNKNOWN] %-46s %s\n' "$2" "$3"; FAIL=$((FAIL+1)) ;;
    FAIL)    printf '  [BLOCKED] %-46s %s\n' "$2" "$3"; FAIL=$((FAIL+1)) ;;
  esac
}

command -v jq >/dev/null 2>&1 || { echo "jq is required" >&2; exit 2; }
[ -f "$ATTEST" ] || { echo "missing $ATTEST" >&2; exit 2; }

[ -n "$CANDIDATE" ] || CANDIDATE="$(jqa '.candidate.tag')"
CAND_COMMIT="$(jqa '.candidate.commit')"

echo "Zalkanes mainnet release gate"
echo "candidate: $CANDIDATE ($CAND_COMMIT)"
echo

# ── 1. Mainnet activation must still be disabled ────────────────────────────
echo "Mechanical gates"

MANIFEST_MAINNET="$(git show "$CAND_COMMIT:protocol/v0.toml" 2>/dev/null \
  | sed -n 's/^mainnet_activation_height = *//p' | tr -d '"')"
SRC_MAINNET="$(git grep -h -E 'MAINNET_ACTIVATION_HEIGHT *: *Option<u32> *=' "$CAND_COMMIT" -- '*.rs' 2>/dev/null \
  | head -1 | sed -E 's/.*= *([^;]*);.*/\1/' | tr -d ' ')"
if [ "$MANIFEST_MAINNET" = "None" ] && [ "$SRC_MAINNET" = "None" ]; then
  report PASS "mainnet activation height is None" "manifest + source agree"
else
  report FAIL "mainnet activation height is None" "manifest='$MANIFEST_MAINNET' source='$SRC_MAINNET'"
fi

# ── 2. Frozen identity: manifest + lockfile digests ─────────────────────────
WANT_MANIFEST="$(jqa '.candidate.protocol_manifest_sha256')"
GOT_MANIFEST="$(git show "$CAND_COMMIT:protocol/v0.toml" 2>/dev/null | shasum -a 256 | cut -d' ' -f1)"
if [ "$WANT_MANIFEST" = "$GOT_MANIFEST" ]; then
  report PASS "protocol manifest hash frozen" "${GOT_MANIFEST:0:16}…"
else
  report FAIL "protocol manifest hash frozen" "expected ${WANT_MANIFEST:0:16}… got ${GOT_MANIFEST:0:16}…"
fi

WANT_LOCK="$(jqa '.candidate.cargo_lock_sha256')"
GOT_LOCK="$(git show "$CAND_COMMIT:Cargo.lock" 2>/dev/null | shasum -a 256 | cut -d' ' -f1)"
if [ "$WANT_LOCK" = "$GOT_LOCK" ]; then
  report PASS "Cargo.lock digest frozen" "${GOT_LOCK:0:16}…"
else
  report FAIL "Cargo.lock digest frozen" "expected ${WANT_LOCK:0:16}… got ${GOT_LOCK:0:16}…"
fi

# ── 3. The candidate tag must exist and still point at the recorded commit ──
TAG_OBJ="$(git rev-parse "$CANDIDATE" 2>/dev/null || true)"
TAG_COMMIT="$(git rev-parse "${CANDIDATE}^{commit}" 2>/dev/null || true)"
WANT_TAG_OBJ="$(jqa '.candidate.tag_object')"
if [ -n "$TAG_COMMIT" ] && [ "$TAG_COMMIT" = "$CAND_COMMIT" ] && [ "$TAG_OBJ" = "$WANT_TAG_OBJ" ]; then
  report PASS "candidate tag identity unchanged" "${TAG_OBJ:0:12}… -> ${TAG_COMMIT:0:12}…"
else
  report FAIL "candidate tag identity unchanged" "tag=${TAG_OBJ:-missing} commit=${TAG_COMMIT:-missing}"
fi

# ── 4. Release tag signature / provenance ───────────────────────────────────
if git verify-tag "$CANDIDATE" >/dev/null 2>&1; then
  report PASS "release tag cryptographically signed" "git verify-tag ok"
else
  report FAIL "release tag cryptographically signed" "$CANDIDATE is unsigned; the FINAL release tag must be signed"
fi

# ── 5. Working tree must be clean ───────────────────────────────────────────
if [ -z "$(git status --porcelain)" ]; then
  report PASS "working tree clean" ""
else
  report FAIL "working tree clean" "$(git status --porcelain | wc -l | tr -d ' ') modified path(s)"
fi

# ── 6. Required CI checks green on the candidate commit ─────────────────────
if [ "$NETWORK" = "1" ] && command -v gh >/dev/null 2>&1; then
  REQUIRED="$(gh api "repos/:owner/:repo/branches/main/protection/required_status_checks" \
    -q '.contexts[]' 2>/dev/null | sort || true)"
  if [ -z "$REQUIRED" ]; then
    report FAIL "required CI checks green on candidate" "could not read branch protection"
  else
    MISSING=""
    while IFS= read -r ctx; do
      [ -n "$ctx" ] || continue
      CONC="$(gh api "repos/:owner/:repo/commits/$CAND_COMMIT/check-runs?per_page=100" \
        -q "[.check_runs[]|select(.name==\"$ctx\")|.conclusion]|last" 2>/dev/null)"
      [ "$CONC" = "success" ] || MISSING="$MISSING $ctx=${CONC:-absent}"
    done <<< "$REQUIRED"
    if [ -z "$MISSING" ]; then
      report PASS "required CI checks green on candidate" "$(echo "$REQUIRED" | wc -l | tr -d ' ') contexts"
    else
      report FAIL "required CI checks green on candidate" "$MISSING"
    fi
  fi
else
  report FAIL "required CI checks green on candidate" "not checked (--no-network or gh missing)"
fi

# ── 7. Reproducible artifacts + SBOM ────────────────────────────────────────
if [ -n "$ARTIFACTS" ] && [ -d "$ARTIFACTS" ]; then
  if [ -f "$ARTIFACTS/SHA256SUMS" ] && (cd "$ARTIFACTS" && shasum -a 256 -c SHA256SUMS >/dev/null 2>&1); then
    report PASS "reproducible artifacts verified" "SHA256SUMS ok"
  else
    report FAIL "reproducible artifacts verified" "SHA256SUMS missing or mismatched in $ARTIFACTS"
  fi
  SBOMS=$(find "$ARTIFACTS" -name '*.cdx.json' 2>/dev/null | wc -l | tr -d ' ')
  if [ "$SBOMS" -gt 0 ]; then
    report PASS "SBOM regenerated/verified" "$SBOMS CycloneDX document(s)"
  else
    report FAIL "SBOM regenerated/verified" "no *.cdx.json under $ARTIFACTS"
  fi
else
  report FAIL "reproducible artifacts verified" "no --artifacts <dir> supplied"
  report FAIL "SBOM regenerated/verified" "no --artifacts <dir> supplied"
fi

# ── 8. Evidence-backed gates ────────────────────────────────────────────────
echo
echo "Evidence-backed gates"
for key in $(jqa '.gates|keys[]'); do
  SAT="$(jqa ".gates[\"$key\"].satisfied")"
  EV="$(jqa ".gates[\"$key\"].evidence")"
  EVH="$(jqa ".gates[\"$key\"].evidence_sha256")"
  NOTE="$(jqa ".gates[\"$key\"].note")"
  PENDING="$(jqa ".gates[\"$key\"].pending_on")"
  if [ "$SAT" != "true" ] && [ "$PENDING" != "null" ] && [ -n "$PENDING" ]; then
    # Unanswerable until the gate it depends on is satisfied.
    if [ "$(jqa ".gates[\"$PENDING\"].satisfied")" != "true" ]; then
      report UNKNOWN "$key" "not answerable until '$PENDING' is satisfied"
      continue
    fi
    report FAIL "$key" "${NOTE}"
  elif [ "$SAT" != "true" ]; then
    report FAIL "$key" "${NOTE}"
  elif [ "$EV" = "null" ] || [ ! -f "$EV" ]; then
    report FAIL "$key" "claims satisfied but evidence file is missing: ${EV}"
  elif [ "$(sha256 "$EV")" != "$EVH" ]; then
    report FAIL "$key" "evidence digest mismatch for $EV"
  else
    report PASS "$key" "$EV"
  fi
done

# ── 9. Candidate-identity binding (see audit/RELEASE-CANDIDATE-LIFECYCLE.md) ─
#
# protocol/v0.toml holds BOTH activation constants and the manifest hash is
# SHA-256 over that file, so setting a mainnet activation height necessarily
# changes the manifest hash and therefore the candidate identity. An audit of
# the testnet candidate does NOT carry over to it.
echo
echo "Candidate-identity binding"

AUDITED_MANIFEST="$(jqa '.audited_candidate.manifest_sha256')"
AUDITED_COMMIT="$(jqa '.audited_candidate.commit')"
SIGNOFF_SAT="$(jqa '.gates.auditor_final_candidate_signoff.satisfied')"
SIGNOFF_COMMIT="$(jqa '.gates.auditor_final_candidate_signoff.final_commit')"
SIGNOFF_MANIFEST="$(jqa '.gates.auditor_final_candidate_signoff.final_manifest_sha256')"

if [ "$AUDITED_MANIFEST" = "null" ] || [ -z "$AUDITED_MANIFEST" ]; then
  report FAIL "audited candidate identity recorded" \
    "no candidate has been externally audited yet"
elif [ "$AUDITED_MANIFEST" = "$GOT_MANIFEST" ] && [ "$AUDITED_COMMIT" = "$CAND_COMMIT" ]; then
  report PASS "final candidate IS the audited candidate" "manifest ${GOT_MANIFEST:0:16}…"
else
  # The candidate differs from what was audited. Only an explicit sign-off on
  # THIS candidate closes the gap.
  if [ "$SIGNOFF_SAT" != "true" ]; then
    report FAIL "auditor sign-off on the FINAL candidate" \
      "candidate manifest ${GOT_MANIFEST:0:16}… differs from audited ${AUDITED_MANIFEST:0:16}…; direct sign-off or written delta confirmation required"
  elif [ "$SIGNOFF_COMMIT" != "$CAND_COMMIT" ] || [ "$SIGNOFF_MANIFEST" != "$GOT_MANIFEST" ]; then
    report FAIL "auditor sign-off on the FINAL candidate" \
      "sign-off names commit ${SIGNOFF_COMMIT:0:12}…/manifest ${SIGNOFF_MANIFEST:0:16}…, not this candidate"
  else
    report PASS "auditor sign-off on the FINAL candidate" \
      "names ${CAND_COMMIT:0:12}… / ${GOT_MANIFEST:0:16}…"
  fi
fi

# ── 10. The activation height itself ────────────────────────────────────────
echo
echo "Final gate"
if [ "$FAIL" -eq 0 ]; then
  report PASS "mainnet activation height may be chosen" "every preceding gate satisfied"
else
  report FAIL "mainnet activation height may be chosen" "$FAIL gate(s) unsatisfied"
fi

echo
if [ "$FAIL" -eq 0 ]; then
  echo "RESULT: all $PASS gates satisfied — mainnet release is permitted."
  exit 0
fi
echo "RESULT: MAINNET RELEASE BLOCKED — $FAIL of $((PASS+FAIL)) gates unsatisfied."
exit 1
