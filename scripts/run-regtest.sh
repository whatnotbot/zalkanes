#!/usr/bin/env bash
# Start Zebra regtest + Zalkanes using Docker Compose.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$SCRIPT_DIR/.."

echo "==> Starting Zalkanes regtest environment..."
cd "$ROOT"
docker compose up --build -d

echo ""
echo "==> Waiting for Zebra regtest to be ready..."
for i in $(seq 1 30); do
  if curl -sf -X POST -H "Content-Type: application/json" \
      --data '{"jsonrpc":"2.0","id":1,"method":"getblockchaininfo","params":[]}' \
      http://127.0.0.1:18232 > /dev/null 2>&1; then
    echo "    Zebra RPC ready."
    break
  fi
  sleep 2
done

echo ""
echo "==> Regtest environment running."
echo "    Zebra RPC:     http://127.0.0.1:18232"
echo "    Zalkanes RPC:  http://127.0.0.1:3030"
echo ""
echo "    Run './scripts/test-e2e.sh' to execute the E2E test suite."
echo "    Run 'docker compose down' to stop."
