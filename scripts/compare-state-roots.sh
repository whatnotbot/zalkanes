#!/usr/bin/env bash
# Compare state roots across multiple independent Zalkanes nodes.
# Usage: ./compare-state-roots.sh http://node-a:3030 http://node-b:3030 http://node-c:3030
set -euo pipefail

if [[ $# -lt 2 ]]; then
  echo "Usage: $0 <node-url> [<node-url>...]"
  exit 1
fi

NODES=("$@")
FAIL=0

echo "==> Comparing state roots across ${#NODES[@]} nodes..."

# Get tip height from first node
TIP_JSON=$(curl -sf -X POST -H "Content-Type: application/json" \
  --data '{"jsonrpc":"2.0","id":1,"method":"zalkanes_getInfo","params":[]}' \
  "${NODES[0]}")
TIP_HEIGHT=$(echo "$TIP_JSON" | python3 -c "import sys,json; d=json.load(sys.stdin); print(d['result']['indexed_height'])" 2>/dev/null || echo "unknown")
echo "    Tip height (node 0): $TIP_HEIGHT"

echo ""
printf "%-8s" "Height"
for node in "${NODES[@]}"; do printf "  %-20s" "$node"; done
echo ""

# Check current root on each node
ROOTS=()
for node in "${NODES[@]}"; do
  ROOT=$(curl -sf -X POST -H "Content-Type: application/json" \
    --data '{"jsonrpc":"2.0","id":1,"method":"zalkanes_getStateRoot","params":[]}' \
    "$node" | python3 -c "import sys,json; print(json.load(sys.stdin)['result'])" 2>/dev/null || echo "error")
  ROOTS+=("$ROOT")
done

printf "%-8s" "$TIP_HEIGHT"
for root in "${ROOTS[@]}"; do printf "  %-20s" "${root:0:20}"; done
echo ""

# Compare all roots
REF="${ROOTS[0]}"
for i in "${!ROOTS[@]}"; do
  if [[ "${ROOTS[$i]}" != "$REF" ]]; then
    echo ""
    echo "ERROR: State root divergence detected at node $i!"
    echo "  Reference: $REF"
    echo "  Node $i:    ${ROOTS[$i]}"
    FAIL=1
  fi
done

echo ""
if [[ $FAIL -eq 0 ]]; then
  echo "==> All nodes agree on state root. ✓"
else
  echo "==> DIVERGENCE DETECTED. This is a consensus bug." >&2
  exit 1
fi
