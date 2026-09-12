#!/usr/bin/env bash
# Live activation-boundary reorg on a private two-node regtest.
#
# Proves that a canonical chain can go below activation -> cross activation ->
# execute Zalkanes state -> reorg back ACROSS the activation boundary -> adopt
# a competing branch -> and reconstruct the correct state from canonical
# history alone.
#
# Everything here uses the real binaries: the real `zalkanes node serve`
# indexing loop, the real transparent DEPLOY/CALL pipeline, and two real pinned
# zebrad nodes. No RocksDB is ever edited by hand.
set -euo pipefail

A=http://127.0.0.1:18232
B=http://127.0.0.1:18242
BIN=./target/release/zalkanes
WASM=target/wasm32-unknown-unknown/release/counter.wasm
ACT="${ZALKANES_REGTEST_ACTIVATION_HEIGHT:-120}"
DATA=/tmp/zalkanes-data
REINDEX=/tmp/zalkanes-reindex
IDX=http://127.0.0.1:3030
IDX2=http://127.0.0.1:3040

rpc () { # url method params
  curl -s -m 120 -X POST "$1" -H 'Content-Type: application/json' \
    -d "{\"jsonrpc\":\"1.0\",\"id\":\"ci\",\"method\":\"$2\",\"params\":$3}"
}
res () { python3 -c 'import json,sys; d=json.load(sys.stdin); sys.exit("RPC error: "+json.dumps(d["error"])) if d.get("error") else print(json.dumps(d.get("result")))'; }
zrpc () { # url method params  (Zalkanes JSON-RPC 2.0)
  curl -s -m 60 -X POST "$1" -H 'Content-Type: application/json' \
    -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"$2\",\"params\":$3}"
}
field () { python3 -c "import json,sys; d=json.load(sys.stdin); print(d['result']['$1'])"; }

height () { rpc "$1" getblockcount '[]' | res; }
bhash  () { rpc "$1" getblockhash "[$2]" | res | tr -d '"'; }

info_height () { zrpc "$IDX" zalkanes_getInfo '[]' | field indexed_height; }
info_root   () { zrpc "$IDX" zalkanes_getInfo '[]' | field state_root; }

start_indexer () { # datadir port logfile
  ZALKANES_DATA_DIR="$1" $BIN node serve --port "$2" --data-dir "$1" > "$3" 2>&1 &
  echo $! > "$3.pid"
}
stop_indexer () { kill "$(cat "$1.pid" 2>/dev/null)" 2>/dev/null || true; sleep 2; }

# Wait until the indexer at $1 reports indexed_height == $2.
wait_indexed () { # url height
  for _ in $(seq 1 120); do
    H=$(zrpc "$1" zalkanes_getInfo '[]' | python3 -c 'import json,sys
d=json.load(sys.stdin)
print(d.get("result",{}).get("indexed_height","?"))' 2>/dev/null || echo "?")
    [ "$H" = "$2" ] && { echo "  indexer reached $2"; return 0; }
    sleep 2
  done
  echo "::error::indexer never reached height $2 (last: ${H:-none})"
  return 1
}

# Move blocks from..to out of $1 (while they are its best chain) into $2.
transplant () { # src dst from to
  for h in $(seq "$3" "$4"); do
    HEX=$(rpc "$1" getblock "[\"$h\", 0]" | res | tr -d '"')
    OUT=$(rpc "$2" submitblock "[\"$HEX\"]")
    echo "$OUT" | grep -q '"error":null' || { echo "::error::submitblock $h failed: $OUT"; return 1; }
  done
}

echo "================= activation boundary = $ACT ================="

# ── 1. Fund, and stop strictly BELOW the activation height ──────────────────
echo "── mining a funded chain that stops below activation ──"
$BIN contract fund --blocks 110 > /dev/null
NOW=$(height $A)
if [ "$NOW" -lt $((ACT - 1)) ]; then
  rpc $A generate "[$((ACT - 1 - NOW))]" > /dev/null
fi
PRE=$(height $A)
echo "chain tip before activation: $PRE (activation is $ACT)"
test "$PRE" -lt "$ACT"

# ── 2. Index the pre-activation chain: state must be provably empty ─────────
rm -rf "$DATA"; mkdir -p "$DATA"
start_indexer "$DATA" 3030 /tmp/indexer.log
wait_indexed "$IDX" "$PRE"
EMPTY_ROOT=$(info_root)
echo "PRE-ACTIVATION: height $(info_height) root $EMPTY_ROOT"

# ── 3. Cross the boundary and execute real Zalkanes state ──────────────────
echo "── crossing the activation boundary ──"
rpc $A generate '[3]' > /dev/null
CROSS=$(height $A)
wait_indexed "$IDX" "$CROSS"
CROSS_ROOT=$(info_root)
echo "AT/ABOVE ACTIVATION (no protocol txs yet): height $CROSS root $CROSS_ROOT"
if [ "$CROSS_ROOT" != "$EMPTY_ROOT" ]; then
  echo "::error::crossing the boundary with no protocol transactions changed the state root"
  exit 1
fi

echo "── deploying the counter above the activation height ──"
$BIN contract deploy "$WASM" --funding transparent --yes 2>&1 | tee /tmp/deploy.log
CID=$(grep -oE '[0-9a-f]{64}' /tmp/deploy.log | tail -1)
rpc $A generate '[2]' > /dev/null
DEPLOYED=$(height $A)
wait_indexed "$IDX" "$DEPLOYED"
DEPLOY_ROOT=$(info_root)
echo "AFTER DEPLOY: contract $CID height $DEPLOYED root $DEPLOY_ROOT"
if [ "$DEPLOY_ROOT" = "$EMPTY_ROOT" ]; then
  echo "::error::the deploy produced no state change"
  exit 1
fi

echo "── calling it ──"
$BIN contract call "$CID" 1 --funding transparent --yes 2>&1 | tail -5
rpc $A generate '[2]' > /dev/null
CALLED=$(height $A)
wait_indexed "$IDX" "$CALLED"
CALL_ROOT=$(info_root)
echo "AFTER CALL: height $CALLED root $CALL_ROOT"
test "$CALL_ROOT" != "$DEPLOY_ROOT"

# ── 4. Fork BELOW the activation height and overtake ────────────────────────
FORK=$((ACT - 2))
echo "── forking at $FORK, which is BELOW the activation height $ACT ──"
transplant $A $B 1 "$FORK"
BH=$(height $B)
test "$BH" = "$FORK"
test "$(bhash $A "$FORK")" = "$(bhash $B "$FORK")"
echo "common ancestor $FORK: $(bhash $B "$FORK")"

NEED=$(( CALLED - FORK + 2 ))
echo "mining $NEED blocks on node B so branch B strictly outweighs branch A"
rpc $B generate "[$NEED]" > /dev/null
BTIP=$(height $B)
echo "branch A tip $CALLED $(bhash $A "$CALLED")"
echo "branch B tip $BTIP $(bhash $B "$BTIP")"
test "$BTIP" -gt "$CALLED"

echo "── submitting branch B to node A ──"
transplant $B $A $((FORK + 1)) "$BTIP"
sleep 3
NEWTIP=$(height $A)
test "$NEWTIP" = "$BTIP"
test "$(bhash $A "$BTIP")" = "$(bhash $B "$BTIP")"
echo "REORG CONFIRMED across the activation boundary: node A tip is now $NEWTIP $(bhash $A "$NEWTIP")"

# ── 5. The indexer must roll back across the boundary and replay ────────────
wait_indexed "$IDX" "$BTIP"
AFTER_ROOT=$(info_root)
echo "AFTER REORG: height $(info_height) root $AFTER_ROOT"
if [ "$AFTER_ROOT" != "$EMPTY_ROOT" ]; then
  echo "::error::stale activated-branch state survived a reorg back across the boundary"
  echo "  expected the empty root $EMPTY_ROOT, got $AFTER_ROOT"
  exit 1
fi
echo "STALE STATE REMOVED: root is the empty-state root again"

# ── 6. Restart must reproduce the same root ─────────────────────────────────
stop_indexer /tmp/indexer.log
start_indexer "$DATA" 3030 /tmp/indexer2.log
wait_indexed "$IDX" "$BTIP"
RESTART_ROOT=$(info_root)
echo "AFTER RESTART: root $RESTART_ROOT"
test "$RESTART_ROOT" = "$AFTER_ROOT"

# ── 7. A clean reindex from an empty database must agree ────────────────────
rm -rf "$REINDEX"; mkdir -p "$REINDEX"
start_indexer "$REINDEX" 3040 /tmp/reindex.log
for _ in $(seq 1 120); do
  H=$(zrpc "$IDX2" zalkanes_getInfo '[]' | python3 -c 'import json,sys
d=json.load(sys.stdin); print(d.get("result",{}).get("indexed_height","?"))' 2>/dev/null || echo "?")
  [ "$H" = "$BTIP" ] && break
  sleep 2
done
CLEAN_ROOT=$(zrpc "$IDX2" zalkanes_getInfo '[]' | field state_root)
echo "CLEAN REINDEX: height $H root $CLEAN_ROOT"
test "$H" = "$BTIP"
test "$CLEAN_ROOT" = "$AFTER_ROOT"

echo
echo "ACTIVATION BOUNDARY REORG PASS"
echo "  activation height     : $ACT"
echo "  pre-activation tip    : $PRE (root $EMPTY_ROOT)"
echo "  contract              : $CID"
echo "  old branch tip        : $CALLED $(bhash $B "$FORK") -> root before reorg $CALL_ROOT"
echo "  fork point (below act): $FORK"
echo "  new branch tip        : $BTIP"
echo "  root after reorg      : $AFTER_ROOT"
echo "  restart root          : $RESTART_ROOT"
echo "  clean reindex root    : $CLEAN_ROOT"
