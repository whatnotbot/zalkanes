#!/usr/bin/env bash
# Two-node public-testnet agreement check for Zalkanes.
#
# Given the JSON-RPC URLs of two (or more) independently initialised Zalkanes
# nodes that follow the SAME validating Zebra chain, require that every node:
#   - advertises the protocol manifest hash of THIS checkout (protocol/v0.toml),
#   - has a live, advancing indexer (`indexer.state` healthy or syncing),
#   - is at the same indexed height and indexed block hash as node 0,
#   - reports the same state root as node 0.
# Roots are only compared at EQUAL heights; comparing roots at different
# heights is meaningless. If the nodes are at different heights the script
# retries for a bounded time to let the laggard catch up.
#
# Usage: scripts/testnet-two-node-check.sh <node-a-url> <node-b-url> [more...]
# Env:   RETRIES (default 20), SLEEP (default 15) between retries.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
[ $# -ge 2 ] || { echo "usage: $0 <node-a-url> <node-b-url> [more...]" >&2; exit 2; }
NODES=("$@")
RETRIES="${RETRIES:-20}"; SLEEP="${SLEEP:-15}"
EXPECT_MANIFEST="$(shasum -a 256 "$ROOT/protocol/v0.toml" | cut -d' ' -f1)"
EXPECT_TESTNET_ACT="$(sed -n 's/^testnet_activation_height = *//p' "$ROOT/protocol/v0.toml")"

info() { curl -sf -m 20 -X POST "$1" -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"zalkanes_getInfo","params":[]}'; }
field() { python3 -c 'import json,sys; r=json.load(sys.stdin)["result"]; v=r
for k in sys.argv[1].split("."): v = v.get(k) if isinstance(v,dict) else None
print("" if v is None else v)' "$1"; }

echo "expected manifest: $EXPECT_MANIFEST (testnet activation $EXPECT_TESTNET_ACT)"
for attempt in $(seq 1 "$RETRIES"); do
  FAIL=0; declare -a H=() B=() R=()
  for i in "${!NODES[@]}"; do
    J=$(info "${NODES[$i]}") || { echo "node $i (${NODES[$i]}): unreachable"; FAIL=1; continue; }
    M=$(echo "$J" | field protocol_manifest_hash); NET=$(echo "$J" | field network)
    ST=$(echo "$J" | field indexer.state); H[$i]=$(echo "$J" | field indexed_height)
    B[$i]=$(echo "$J" | field indexed_block_hash); R[$i]=$(echo "$J" | field state_root)
    TIP=$(echo "$J" | field chain_tip_height)
    echo "node $i: network=$NET height=${H[$i]} tip=$TIP indexer=${ST:-n/a} root=${R[$i]:0:16}… block=${B[$i]:0:16}…"
    [ "$NET" = "test" ] || { echo "  FAIL: network is '$NET', expected 'test'"; FAIL=1; }
    [ "$M" = "$EXPECT_MANIFEST" ] || { echo "  FAIL: manifest $M != expected"; FAIL=1; }
    case "${ST:-}" in healthy|syncing) ;; *) echo "  FAIL: indexer state '${ST:-missing}' (need healthy or syncing)"; FAIL=1 ;; esac
  done
  if [ "$FAIL" = 0 ]; then
    SAME=1
    for i in "${!NODES[@]}"; do
      [ "${H[$i]}" = "${H[0]}" ] || SAME=0
    done
    if [ "$SAME" = 1 ]; then
      for i in "${!NODES[@]}"; do
        [ "${B[$i]}" = "${B[0]}" ] || { echo "DIVERGENCE: node $i indexed block hash differs at height ${H[0]}"; exit 1; }
        [ "${R[$i]}" = "${R[0]}" ] || { echo "DIVERGENCE: node $i state root differs at height ${H[0]}"; exit 1; }
      done
      echo "AGREEMENT: ${#NODES[@]} nodes at height ${H[0]}, block ${B[0]}, root ${R[0]}"
      exit 0
    fi
    echo "  heights differ; waiting for the laggard (attempt $attempt/$RETRIES)"
  else
    echo "  (attempt $attempt/$RETRIES)"
  fi
  sleep "$SLEEP"
done
echo "FAIL: nodes did not reach agreement within $((RETRIES*SLEEP))s" >&2
exit 1
