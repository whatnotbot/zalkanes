#!/usr/bin/env bash
# Public-testnet platform acceptance for Zalkanes v0 (counter example).
#
# BROADCASTS REAL TESTNET TRANSACTIONS. It refuses to run unless the owner
# has explicitly authorised it by setting ZALKANES_TESTNET_ACCEPTANCE=yes.
#
# Sequence (docs/release/testnet-rc2-deployment.md §5):
#   1. build the counter          2. deploy a fresh counter (PREPARE + DEPLOY)
#   3. verify the ContractId       4. VIEW get == 0
#   5. CALL increment              6. VIEW get == 1
#   7. CALL increment              8. VIEW get == 2
# Steps 9-12 (restart node A, clean-reindex node B, two-node agreement) are
# operator actions; run scripts/testnet-two-node-check.sh afterwards.
#
# Required environment (see docs/developers/testnet.md):
#   ZALKANES_NETWORK=testnet
#   ZALKANES_ZCASH_RPC_URL     your own synced Zebra testnet validator
#   ZALKANES_URL               the Zalkanes node to VIEW against (node A)
#   ZALKANES_SIGNING_KEY       fresh 32-byte hex key (transparent funding), and
#   ZALKANES_FUNDING_TXID/VOUT the faucet payment to that key's address
#   FUNDING=shielded           optional: shielded PREPARE/CALLs via the wallet
#                              (needs a funded, scanned, unlocked wallet)
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"; cd "$ROOT"
[ "${ZALKANES_TESTNET_ACCEPTANCE:-}" = "yes" ] || {
  echo "refusing: set ZALKANES_TESTNET_ACCEPTANCE=yes only when the owner has authorised broadcasting testnet transactions" >&2; exit 2; }
[ "${ZALKANES_NETWORK:-}" = "testnet" ] || { echo "ZALKANES_NETWORK must be testnet" >&2; exit 2; }
: "${ZALKANES_ZCASH_RPC_URL:?set to your own Zebra testnet RPC}"
: "${ZALKANES_URL:?set to the Zalkanes node JSON-RPC URL}"
FUNDING="${FUNDING:-transparent}"
export PATH="$ROOT/target/release:$PATH"
OUT="${OUT:-$ROOT/.testnet-acceptance}"; mkdir -p "$OUT"
EVIDENCE="$OUT/evidence-$(date -u +%Y%m%dT%H%M%SZ).log"
log() { echo "$*" | tee -a "$EVIDENCE"; }
view() { zalkanes contract view "$1" 2 "" --rpc-url "$ZALKANES_URL"; }
value() { python3 -c 'import json,sys; r=json.load(sys.stdin)["result"]; print(r["output_hex"] if r["success"] else "FAILED:"+str(r["error"]))'; }
info() { curl -sf -m 20 -X POST "$ZALKANES_URL" -H 'Content-Type: application/json' -d '{"jsonrpc":"2.0","id":1,"method":"zalkanes_getInfo","params":[]}'; }
require() { [ "$1" = "$2" ] || { log "FAIL: $3 (got $1, expected $2)"; exit 1; }; }

log "== Zalkanes public-testnet acceptance $(date -u +%FT%TZ) commit $(git rev-parse HEAD)"
log "manifest (local): $(shasum -a 256 protocol/v0.toml | cut -d' ' -f1)"
log "node getInfo: $(info)"
ACT=$(sed -n 's/^testnet_activation_height = *//p' protocol/v0.toml)
H=$(info | python3 -c 'import json,sys; print(json.load(sys.stdin)["result"]["indexed_height"])')
[ "$H" -ge "$ACT" ] || { log "FAIL: node indexed height $H is below activation $ACT; wait for activation"; exit 1; }

log "== 1. build the counter"
zalkanes contract build --manifest-path ./contracts/counter | tee -a "$EVIDENCE"
WASM=./contracts/counter/target/wasm32-unknown-unknown/release/counter.wasm
log "wasm sha256: $(shasum -a 256 "$WASM" | cut -d' ' -f1)"

log "== 2. deploy ($FUNDING PREPARE, transparent DEPLOY)"
zalkanes contract deploy "$WASM" --funding "$FUNDING" --yes --wait 2>&1 | tee "$OUT/deploy.log" | tee -a "$EVIDENCE"
CID=$(sed -n 's/^ContractId: //p' "$OUT/deploy.log"); [ ${#CID} -eq 64 ] || { log "FAIL: no ContractId"; exit 1; }

log "== 3. verify the ContractId is indexed with the built code hash"
IDX=$(curl -sf -X POST "$ZALKANES_URL" -H 'Content-Type: application/json' -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"zalkanes_getContract\",\"params\":[\"$CID\"]}")
log "getContract: $IDX"
require "$(echo "$IDX" | python3 -c 'import json,sys; print(json.load(sys.stdin)["result"]["code_hash"])')" "$(shasum -a 256 "$WASM" | cut -d' ' -f1)" "indexed code hash equals built wasm"

log "== 4. VIEW get == 0"; V=$(view "$CID" | tee -a "$EVIDENCE" | value); require "$V" "0000000000000000" "initial value"
log "== 5. CALL increment"; zalkanes contract call "$CID" 1 "" --funding "$FUNDING" --yes --wait 2>&1 | tee -a "$EVIDENCE" | grep -E "^(CALL mined|execution)"
log "== 6. VIEW get == 1"; V=$(view "$CID" | tee -a "$EVIDENCE" | value); require "$V" "0000000000000001" "after first increment"
log "== 7. CALL increment"; zalkanes contract call "$CID" 1 "" --funding "$FUNDING" --yes --wait 2>&1 | tee -a "$EVIDENCE" | grep -E "^(CALL mined|execution)"
log "== 8. VIEW get == 2"; V=$(view "$CID" | tee -a "$EVIDENCE" | value); require "$V" "0000000000000002" "after second increment"
log "final getInfo: $(info)"
log "ACCEPTANCE STEPS 1-8 PASS  contract=$CID  evidence=$EVIDENCE"
log "next: restart node A, clean-reindex node B, then scripts/testnet-two-node-check.sh <A> <B> and re-run VIEW on both"
