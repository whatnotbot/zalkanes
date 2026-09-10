#!/usr/bin/env bash
# End-to-end test of the Zalkanes deterministic pipeline.
#
# Phase 1 (no external node — always runs):
#   Build the counter contract and run the Rust integration tests, which drive
#   the REAL production block processor over an in-memory StateStore:
#     deploy -> increment -> increment -> view get()==2
#     state root changes at each step
#     RocksDB restart persistence
#     fresh-reindex determinism
#     reorg rollback == clean replay
#
# Phase 2 (requires a running Zebra regtest + wallet; see run-regtest.sh):
#   Real Zcash broadcast, mining, indexing, and cross-node root comparison.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$SCRIPT_DIR/.."

echo "==> Building counter contract..."
cd "$ROOT"
cargo build --release --target wasm32-unknown-unknown \
  --manifest-path contracts/counter/Cargo.toml 2>&1 | tail -5

WASM=$(find contracts/counter/target/wasm32-unknown-unknown/release -name "counter.wasm" | head -1)
if [[ -z "$WASM" ]]; then
  echo "ERROR: counter.wasm not found" >&2; exit 1
fi
echo "    WASM: $WASM ($(wc -c < "$WASM") bytes)"
echo "    SHA256: $(sha256sum "$WASM" | awk '{print $1}')"

echo ""
echo "==> Running deterministic pipeline tests (deploy/call/view, restart, reindex, reorg)..."
cargo test -p zalkanes-testkit --test e2e 2>&1 | tail -20

echo ""
echo "==> Running workspace unit tests..."
cargo test --workspace 2>&1 | grep -E "test result:" | awk '{s+=$4} END {print "    " s " tests passed"}'

echo ""
echo "==> Phase 1 complete."
if [[ -n "${ZEBRA_RPC_URL:-}" && -n "${ZALKANES_RPC_URL:-}" ]]; then
  echo "==> Phase 2: live regtest E2E (requires wallet key in env)."
  echo "    See scripts/run-regtest.sh and docs/tx.md."
else
  echo "==> Skipping Phase 2 (no Zebra/Zalkanes RPC configured)."
  echo "    Set ZEBRA_RPC_URL + ZALKANES_RPC_URL to run live broadcast E2E."
fi
