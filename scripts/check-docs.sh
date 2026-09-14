#!/usr/bin/env bash
# Documentation consistency gate. Needs only python3 and git; no Rust toolchain.
#   scripts/check-docs.sh          check
#   scripts/check-docs.sh --fix    also regenerate the limits table from protocol/v0.toml
#   scripts/check-docs.sh --no-git skip the git tag identity check
set -euo pipefail
exec python3 "$(dirname "${BASH_SOURCE[0]}")/check-docs.py" "$@"
