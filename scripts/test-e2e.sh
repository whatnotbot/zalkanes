#!/usr/bin/env bash
# Kept for compatibility. The regtest end-to-end acceptance now lives in
# scripts/dev-quickstart-test.sh, which runs exactly the commands documented
# in docs/developers/quickstart.md.
exec "$(dirname "${BASH_SOURCE[0]}")/dev-quickstart-test.sh" "$@"
