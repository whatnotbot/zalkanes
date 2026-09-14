#!/usr/bin/env bash
# Start or stop a local Zcash regtest environment for Zalkanes development:
# one Zebra (zebrad) regtest node plus one Zalkanes indexer.
#
# Two modes, chosen automatically (override with ZALKANES_REGTEST_MODE):
#
#   native  A `zebrad` binary is on PATH (or $ZEBRAD points at one) and the
#           Zalkanes CLI has been built (`cargo build --release`). Both
#           processes run directly on this machine. This is the mode that
#           scripts/dev-quickstart-test.sh and the developer docs exercise.
#
#   docker  No zebrad binary, but `docker compose` is available: brings up
#           docker-compose.yml (pinned zfnd/zebra image + a Zalkanes
#           container). Provided as a convenience; not exercised by the
#           acceptance script.
#
# Usage:
#   scripts/run-regtest.sh [start]        start Zebra + Zalkanes (idempotent)
#   scripts/run-regtest.sh stop           stop both
#   scripts/run-regtest.sh restart-node   restart only the Zalkanes indexer
#   scripts/run-regtest.sh restart-node --clean   same, after deleting its state (clean reindex)
#   scripts/run-regtest.sh status         show what is running
#   scripts/run-regtest.sh env            print the `export` lines the CLI needs
#
# Environment:
#   ZALKANES_REGTEST_DIR    working directory (default ./.regtest, git-ignored)
#   ZALKANES_REGTEST_FRESH  set to 1 to wipe the chain and the indexer state first
#   ZEBRAD                  path to zebrad (default: `zebrad` on PATH)
#   ZALKANES_BIN            path to zalkanes (default ./target/release/zalkanes)
#
# Ports (native and docker): Zebra RPC 18232, Zebra P2P 18233,
# Zalkanes JSON-RPC 3030, Zalkanes health 3031.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$ROOT"

DIR="${ZALKANES_REGTEST_DIR:-$ROOT/.regtest}"
ZEBRAD="${ZEBRAD:-zebrad}"
ZALKANES_BIN="${ZALKANES_BIN:-$ROOT/target/release/zalkanes}"
ZEBRA_RPC="http://127.0.0.1:18232"
ZALKANES_RPC="http://127.0.0.1:3030"
CMD="${1:-start}"

zebra_rpc() { # method params-json
  curl -s -m 5 -X POST "$ZEBRA_RPC" -H 'Content-Type: application/json' \
    -d "{\"jsonrpc\":\"1.0\",\"id\":\"regtest\",\"method\":\"$1\",\"params\":${2:-[]}}"
}
zalkanes_rpc() { # method params-json
  curl -s -m 5 -X POST "$ZALKANES_RPC" -H 'Content-Type: application/json' \
    -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"$1\",\"params\":${2:-[]}}"
}
zebra_up()    { zebra_rpc getblockchaininfo | grep -q '"chain"'; }
zalkanes_up() { zalkanes_rpc zalkanes_getInfo | grep -q '"state_root"'; }

wait_for() { # description seconds check-fn
  local i
  for i in $(seq 1 "$2"); do
    if "$3"; then echo "    $1 ready (${i}s)"; return 0; fi
    sleep 1
  done
  echo "ERROR: $1 did not come up within $2s" >&2
  return 1
}

detect_mode() {
  if [ -n "${ZALKANES_REGTEST_MODE:-}" ]; then echo "$ZALKANES_REGTEST_MODE"; return; fi
  if command -v "$ZEBRAD" >/dev/null 2>&1; then echo native; return; fi
  if docker compose version >/dev/null 2>&1; then echo docker; return; fi
  echo none
}

print_env() {
  cat <<ENV
export ZALKANES_NETWORK=regtest
export ZALKANES_ZCASH_RPC_URL=$ZEBRA_RPC
export ZALKANES_URL=$ZALKANES_RPC
export ZALKANES_DATA_DIR=$DIR/zalkanes-data
ENV
}

pid_alive() { [ -f "$1" ] && kill -0 "$(cat "$1")" 2>/dev/null; }

# ── native mode ──────────────────────────────────────────────────────────────

write_zebra_config() {
  # Same shape as deploy/zebra/zebra-regtest.toml and the CI regtest jobs,
  # which is known-good for the pinned zebrad v6.3.0: every network upgrade
  # active from height 1, no peers, RPC on loopback without cookie auth.
  mkdir -p "$DIR/zebra"
  cat > "$DIR/zebrad.toml" <<ZCFG
[network]
network = { regtest = true, params = { activation_heights = { NU5 = 1, NU6 = 1, "NU6.1" = 1, "NU6.2" = 1, "NU6.3" = 1 } } }
listen_addr = "127.0.0.1:18233"
initial_mainnet_peers = []
initial_testnet_peers = []
cache_dir = "$DIR/zebra/net"

[state]
cache_dir = "$DIR/zebra/state"
ephemeral = false

[sync]
checkpoint_verify_concurrency_limit = 2
download_concurrency_limit = 2

[consensus]
checkpoint_sync = false

[rpc]
listen_addr = "127.0.0.1:18232"
enable_cookie_auth = false

[mining]
internal_miner = false
miner_address = "t27eWDgjFYJGVXmzrXeVjnb5J3uXDM9xH9v"
ZCFG
}

start_zebra_native() {
  if pid_alive "$DIR/zebrad.pid"; then echo "    zebrad already running (pid $(cat "$DIR/zebrad.pid"))"; return; fi
  # A stray zebrad from a previous session would hold the ports.
  pkill -f "zebrad -c $DIR/zebrad.toml" 2>/dev/null || true
  write_zebra_config
  "$ZEBRAD" -c "$DIR/zebrad.toml" start > "$DIR/zebrad.log" 2>&1 &
  echo $! > "$DIR/zebrad.pid"
  wait_for "Zebra regtest RPC ($ZEBRA_RPC)" 90 zebra_up || { tail -30 "$DIR/zebrad.log"; exit 1; }
}

start_zalkanes_native() {
  if pid_alive "$DIR/zalkanes.pid"; then echo "    zalkanes already running (pid $(cat "$DIR/zalkanes.pid"))"; return; fi
  [ -x "$ZALKANES_BIN" ] || { echo "ERROR: $ZALKANES_BIN not found; run: cargo build --release" >&2; exit 1; }
  mkdir -p "$DIR/zalkanes-data"
  ZALKANES_NETWORK=regtest \
  ZALKANES_RPC_URL="$ZEBRA_RPC" \
  ZALKANES_DATA_DIR="$DIR/zalkanes-data" \
  RUST_LOG="${RUST_LOG:-zalkanes=info}" \
    "$ZALKANES_BIN" node serve --port 3030 > "$DIR/zalkanes.log" 2>&1 &
  echo $! > "$DIR/zalkanes.pid"
  wait_for "Zalkanes JSON-RPC ($ZALKANES_RPC)" 60 zalkanes_up || { tail -30 "$DIR/zalkanes.log"; exit 1; }
}

stop_pid() { # pidfile name
  if pid_alive "$1"; then
    kill "$(cat "$1")" 2>/dev/null || true
    for _ in $(seq 1 30); do pid_alive "$1" || break; sleep 1; done
    pid_alive "$1" && kill -9 "$(cat "$1")" 2>/dev/null || true
    echo "    stopped $2"
  fi
  rm -f "$1"
}

native_start() {
  echo "==> Starting Zalkanes regtest environment (native mode) in $DIR"
  if [ "${ZALKANES_REGTEST_FRESH:-0}" = "1" ]; then
    echo "    ZALKANES_REGTEST_FRESH=1: wiping the previous chain and state"
    stop_pid "$DIR/zalkanes.pid" zalkanes
    stop_pid "$DIR/zebrad.pid" zebrad
    rm -rf "$DIR"
  fi
  mkdir -p "$DIR"
  start_zebra_native
  start_zalkanes_native
  echo
  echo "==> Regtest environment running."
  echo "    Zebra RPC:     $ZEBRA_RPC   (log: $DIR/zebrad.log)"
  echo "    Zalkanes RPC:  $ZALKANES_RPC    (log: $DIR/zalkanes.log)"
  echo
  echo "    Export these before using the CLI (or: eval \"\$(scripts/run-regtest.sh env)\"):"
  print_env | sed 's/^/      /'
  echo
  echo "    Stop with: scripts/run-regtest.sh stop"
}

native_status() {
  if pid_alive "$DIR/zebrad.pid"; then
    echo "zebrad:   running (pid $(cat "$DIR/zebrad.pid")), height $(zebra_rpc getblockcount | sed -n 's/.*"result":\([0-9]*\).*/\1/p')"
  else echo "zebrad:   stopped"; fi
  if pid_alive "$DIR/zalkanes.pid"; then
    echo "zalkanes: running (pid $(cat "$DIR/zalkanes.pid"))"
    zalkanes_rpc zalkanes_getInfo; echo
  else echo "zalkanes: stopped"; fi
}

# ── docker mode ──────────────────────────────────────────────────────────────

docker_start() {
  echo "==> Starting Zalkanes regtest environment (docker compose mode)"
  docker compose up --build -d
  wait_for "Zebra regtest RPC ($ZEBRA_RPC)" 120 zebra_up
  wait_for "Zalkanes JSON-RPC ($ZALKANES_RPC)" 120 zalkanes_up
  echo
  echo "==> Regtest environment running (containers). Export these before using the CLI:"
  print_env | grep -v ZALKANES_DATA_DIR | sed 's/^/      /'
  echo "    Stop with: scripts/run-regtest.sh stop"
}

# ── dispatch ─────────────────────────────────────────────────────────────────

MODE="$(detect_mode)"
case "$CMD" in
  env) print_env ;;
  start)
    case "$MODE" in
      native) native_start ;;
      docker) docker_start ;;
      *) echo "ERROR: neither a zebrad binary nor docker compose is available." >&2
         echo "       See docs/developers/local-regtest.md for how to obtain zebrad v6.3.0." >&2
         exit 1 ;;
    esac ;;
  stop)
    case "$MODE" in
      docker) docker compose down ;;
      *) stop_pid "$DIR/zalkanes.pid" zalkanes; stop_pid "$DIR/zebrad.pid" zebrad ;;
    esac ;;
  restart-node)
    case "$MODE" in
      docker)
        [ "${2:-}" = "--clean" ] && { echo "ERROR: --clean is only supported in native mode" >&2; exit 2; }
        docker compose restart zalkanes; wait_for "Zalkanes JSON-RPC" 60 zalkanes_up ;;
      *)
        stop_pid "$DIR/zalkanes.pid" zalkanes
        if [ "${2:-}" = "--clean" ]; then
          echo "    --clean: deleting $DIR/zalkanes-data (the node will reindex from Zebra)"
          rm -rf "$DIR/zalkanes-data"
        fi
        start_zalkanes_native ;;
    esac ;;
  status)
    case "$MODE" in
      docker) docker compose ps ;;
      *) native_status ;;
    esac ;;
  *) echo "usage: $0 [start|stop|restart-node [--clean]|status|env]" >&2; exit 2 ;;
esac
