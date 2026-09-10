#!/usr/bin/env bash
# Build counter and key-value contracts with reproducible flags.
# Expected: two isolated clean builds produce identical SHA256 hashes.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

export SOURCE_DATE_EPOCH=0
export RUSTFLAGS="-C link-arg=-s"
export CARGO_HOME="$ROOT/.cargo-reproducible"

echo "==> Reproducible contract build"

for contract in counter key-value token; do
  echo ""
  echo "--- $contract ---"
  cargo build --release --target wasm32-unknown-unknown \
    -p "$contract" 2>&1 | tail -3

  WASM=$(find target/wasm32-unknown-unknown/release -name "${contract//-/_}.wasm" 2>/dev/null | head -1)
  [[ -z "$WASM" ]] && WASM=$(find target/wasm32-unknown-unknown/release -name "*.wasm" | head -1)
  if [[ -n "$WASM" ]]; then
    HASH=$(sha256sum "$WASM" | awk '{print $1}')
    echo "    SHA256: $HASH  $WASM"
  fi
done

echo ""
echo "==> Done. Compare hashes across two clean containers for reproducibility."
