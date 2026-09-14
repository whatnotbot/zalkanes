#!/usr/bin/env bash
# Clean-machine developer acceptance test.
#
# Proves that the flow documented in docs/developers/quickstart.md works, by
# running EXACTLY the commands shown there (scripts/check-docs.sh fails CI if
# the two drift apart), then the persistence checks from
# docs/developers/local-regtest.md:
#
#   1. build the workspace              cargo build --release
#   2. start a FRESH regtest             ./scripts/run-regtest.sh start
#   3. build counter.wasm                zalkanes contract build
#   4. fund + deploy the counter         zalkanes contract fund / deploy
#   5. increment twice                   zalkanes contract call
#   6. view get() and require == 2       zalkanes contract view
#   7. restart the node: same root, same view
#   8. clean reindex from Zebra: same root
#
# Requirements: the pinned Rust toolchain and a `zebrad` v6.3.0 on PATH
# (see docs/developers/local-regtest.md). Set KEEP_REGTEST=1 to leave the
# environment running afterwards.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
OUT="${ZALKANES_REGTEST_DIR:-$ROOT/.regtest}"

step() { echo; echo "==> $*"; }
fail() { echo "FAIL: $*" >&2; exit 1; }
zrpc() { # method params-json
  curl -s -m 10 -X POST http://127.0.0.1:3030 -H 'Content-Type: application/json' \
    -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"$1\",\"params\":${2:-[]}}"
}
json_field() { python3 -c 'import json,sys; d=json.load(sys.stdin); v=d["result"]; print(v[sys.argv[1]] if isinstance(v,dict) else v)' "$1"; }
wait_indexed() { # height
  for _ in $(seq 1 120); do
    H=$(zrpc zalkanes_getInfo | json_field indexed_height 2>/dev/null || echo "")
    [ "$H" = "$1" ] && return 0
    sleep 1
  done
  fail "node did not reach height $1 (last: ${H:-none})"
}

step "1. build the workspace"
cargo build --release
export PATH="$PWD/target/release:$PATH"

step "2. start a fresh regtest (Zebra + Zalkanes)"
export ZALKANES_REGTEST_FRESH=1
./scripts/run-regtest.sh start
eval "$(./scripts/run-regtest.sh env)"

step "3. build the counter contract"
zalkanes contract build --manifest-path ./contracts/counter
WASM=./contracts/counter/target/wasm32-unknown-unknown/release/counter.wasm
[ -f "$WASM" ] || fail "$WASM was not produced"

step "4. fund and deploy"
zalkanes contract fund
mkdir -p "$OUT"
zalkanes contract deploy ./contracts/counter/target/wasm32-unknown-unknown/release/counter.wasm --funding transparent --yes --wait | tee "$OUT/deploy.log"
CONTRACT_ID=$(sed -n 's/^ContractId: //p' "$OUT/deploy.log")
[ ${#CONTRACT_ID} -eq 64 ] || fail "no ContractId in deploy output"
echo "ContractId: $CONTRACT_ID"

step "5. increment twice"
zalkanes contract call "$CONTRACT_ID" 1 "" --funding transparent --yes --wait
zalkanes contract call "$CONTRACT_ID" 1 "" --funding transparent --yes --wait

step "6. view get()"
VIEW=$(zalkanes contract view "$CONTRACT_ID" 2 "" --rpc-url http://127.0.0.1:3030)
echo "$VIEW"
VALUE=$(echo "$VIEW" | json_field output_hex)
[ "$VALUE" = "0000000000000002" ] || fail "expected output_hex 0000000000000002, got $VALUE"
echo "counter == 2"

step "7. restart the node: state must persist"
TIP=$(zrpc zalkanes_getInfo | json_field indexed_height)
ROOT_BEFORE=$(zrpc zalkanes_getStateRoot | json_field result)
./scripts/run-regtest.sh restart-node
wait_indexed "$TIP"
ROOT_AFTER=$(zrpc zalkanes_getStateRoot | json_field result)
[ "$ROOT_BEFORE" = "$ROOT_AFTER" ] || fail "state root changed across restart: $ROOT_BEFORE != $ROOT_AFTER"
VALUE=$(zalkanes contract view "$CONTRACT_ID" 2 "" --rpc-url http://127.0.0.1:3030 | json_field output_hex)
[ "$VALUE" = "0000000000000002" ] || fail "view after restart returned $VALUE"
echo "root unchanged after restart: $ROOT_AFTER"

step "8. clean reindex from Zebra: same root"
./scripts/run-regtest.sh restart-node --clean
wait_indexed "$TIP"
ROOT_REINDEX=$(zrpc zalkanes_getStateRoot | json_field result)
[ "$ROOT_BEFORE" = "$ROOT_REINDEX" ] || fail "clean reindex root differs: $ROOT_BEFORE != $ROOT_REINDEX"
VALUE=$(zalkanes contract view "$CONTRACT_ID" 2 "" --rpc-url http://127.0.0.1:3030 | json_field output_hex)
[ "$VALUE" = "0000000000000002" ] || fail "view after reindex returned $VALUE"
echo "root identical after clean reindex: $ROOT_REINDEX"

if [ "${KEEP_REGTEST:-0}" != "1" ]; then
  step "stop"
  ./scripts/run-regtest.sh stop
fi

echo
echo "DEV QUICKSTART PASS"
echo "  contract   : $CONTRACT_ID"
echo "  final value: 2"
echo "  height     : $TIP"
echo "  state root : $ROOT_REINDEX (persisted == restart == clean reindex)"
