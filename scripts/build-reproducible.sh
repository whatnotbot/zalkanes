#!/usr/bin/env bash
# Build reference contracts with reproducible flags.
# Expected: two isolated clean builds produce identical SHA256 hashes.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

export SOURCE_DATE_EPOCH=0
export RUSTFLAGS="-C link-arg=-s"
export CARGO_HOME="$ROOT/.cargo-reproducible"

echo "==> Reproducible contract build"

for contract in counter key-value token caller; do
  echo ""
  echo "--- $contract ---"
  manifest="contracts/$contract/Cargo.toml"
  cargo build --release --target wasm32-unknown-unknown \
    --manifest-path "$manifest" 2>&1 | tail -3

  # Contract crates are standalone, so artifacts live under their own target dir.
  bin_name="${contract//-/_}"
  WASM=$(find "contracts/$contract/target/wasm32-unknown-unknown/release" \
    -name "${bin_name}.wasm" 2>/dev/null | head -1)
  [[ -z "$WASM" ]] && WASM=$(find "contracts/$contract/target/wasm32-unknown-unknown/release" -name "*.wasm" 2>/dev/null | head -1)
  if [[ -n "$WASM" ]]; then
    HASH=$(sha256sum "$WASM" | awk '{print $1}')
    echo "    SHA256: $HASH  $WASM"
  fi
done

echo ""
echo "==> Done. Compare hashes across two clean containers for reproducibility."
