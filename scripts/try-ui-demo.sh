#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
export DATUM_CONNECT_REPO="${DATUM_CONNECT_REPO:-$REPO_ROOT/.datum-connect-dev}"

ORIGIN="${ORIGIN:-datumconnect.test}"
DNS_BIND="${DNS_BIND:-127.0.0.1:53535}"
GATEWAY_PORT="${GATEWAY_PORT:-8080}"
ORIGIN_PORT="${ORIGIN_PORT:-5173}"
TUNNEL_PORT="${TUNNEL_PORT:-8888}"
DX_PORT="${DX_PORT:-8081}"
DNS_DATA="${DNS_DATA:-$REPO_ROOT/dns-dev.yml}"

cleanup() {
  pkill -f "datum-connect dns-dev serve" || true
  pkill -f "datum-connect serve" || true
  pkill -f "datum-connect gateway" || true
  pkill -f "datum-connect tunnel-dev" || true
  pkill -f "openssl s_server -accept ${ORIGIN_PORT}" || true
}
trap cleanup EXIT

kill_port() {
  local port="$1"
  local pids
  pids="$(lsof -tiTCP:"$port" -sTCP:LISTEN 2>/dev/null || true)"
  if [[ -n "$pids" ]]; then
    kill $pids 2>/dev/null || true
  fi
}

kill_port "53535"
kill_port "${GATEWAY_PORT}"
kill_port "${ORIGIN_PORT}"
kill_port "${TUNNEL_PORT}"

if ! command -v openssl >/dev/null 2>&1; then
  echo "openssl is required for the local HTTPS origin."
  exit 1
fi

if ! command -v dx >/dev/null 2>&1; then
  echo "dx (dioxus-cli) is not installed; GUI will not be started automatically."
fi

echo "Starting dns-dev server..."
(cd "$REPO_ROOT" && cargo run -p datum-connect -- dns-dev serve \
  --origin "$ORIGIN" \
  --bind "$DNS_BIND" \
  --data "$DNS_DATA") >/tmp/datum-connect-dns-dev.log 2>&1 &

echo "Starting local HTTPS origin on ${ORIGIN_PORT}..."
openssl req -x509 -nodes -newkey rsa:2048 -days 1 \
  -keyout /tmp/iroh-dev.key -out /tmp/iroh-dev.crt \
  -subj "/CN=localhost" >/tmp/datum-connect-openssl.log 2>&1
openssl s_server -accept "$ORIGIN_PORT" -cert /tmp/iroh-dev.crt -key /tmp/iroh-dev.key -www \
  >/tmp/datum-connect-origin.log 2>&1 &

echo "Starting gateway in forward mode..."
(cd "$REPO_ROOT" && cargo run -p datum-connect -- gateway \
  --port "$GATEWAY_PORT" \
  --mode forward \
  --discovery dns \
  --dns-origin "$ORIGIN" \
  --dns-resolver "$DNS_BIND") >/tmp/datum-connect-gateway.log 2>&1 &

echo "Starting listen node..."
(cd "$REPO_ROOT" && cargo run -p datum-connect -- serve) >/tmp/datum-connect-serve.log 2>&1 &

echo "Waiting for listen node output..."
ENDPOINT_ID=""
V4_ADDR=""
V6_ADDR=""
for _ in $(seq 1 240); do
  if [[ -z "$ENDPOINT_ID" ]]; then
    ENDPOINT_ID="$(grep -Eo 'listening as [0-9a-f]+' /tmp/datum-connect-serve.log | awk '{print $3}' | tail -n1 || true)"
  fi
  if [[ -z "$V4_ADDR" ]]; then
    V4_ADDR="$(grep -Eo '0.0.0.0:[0-9]+' /tmp/datum-connect-serve.log | tail -n1 || true)"
    V4_ADDR="${V4_ADDR/0.0.0.0/127.0.0.1}"
  fi
  if [[ -z "$V6_ADDR" ]]; then
    V6_ADDR="$(grep -Eo '\\[::\\]:[0-9]+' /tmp/datum-connect-serve.log | tail -n1 || true)"
    V6_ADDR="${V6_ADDR/\\[::\\]/[::1]}"
  fi
  if [[ -n "$ENDPOINT_ID" && -n "$V4_ADDR" ]]; then
    break
  fi
  sleep 0.25
done

if [[ -z "$ENDPOINT_ID" || -z "$V4_ADDR" ]]; then
  echo "Failed to detect endpoint id or bound sockets."
  echo "serve log tail:"
  tail -n 50 /tmp/datum-connect-serve.log || true
  echo "gateway log tail:"
  tail -n 20 /tmp/datum-connect-gateway.log || true
  echo "dns-dev log tail:"
  tail -n 20 /tmp/datum-connect-dns-dev.log || true
  exit 1
fi

echo "Publishing TXT records via dns-dev..."
CMD=(cargo run -p datum-connect -- dns-dev upsert \
  --origin "$ORIGIN" \
  --data "$DNS_DATA" \
  --endpoint-id "$ENDPOINT_ID" \
  --addr "$V4_ADDR")
if [[ -n "$V6_ADDR" ]]; then
  CMD+=(--addr "$V6_ADDR")
fi
(cd "$REPO_ROOT" && "${CMD[@]}") >/tmp/datum-connect-dns-upsert.log 2>&1

echo "Starting tunnel-dev entrypoint on ${TUNNEL_PORT}..."
(cd "$REPO_ROOT" && cargo run -p datum-connect -- tunnel-dev \
  --gateway "127.0.0.1:${GATEWAY_PORT}" \
  --node-id "$ENDPOINT_ID" \
  --target-host 127.0.0.1 \
  --target-port "${ORIGIN_PORT}" \
  --listen "127.0.0.1:${TUNNEL_PORT}") >/tmp/datum-connect-tunnel-dev.log 2>&1 &

if command -v dx >/dev/null 2>&1; then
  echo "Starting GUI..."
  (cd "$REPO_ROOT/ui" && dx serve --platform desktop --port "$DX_PORT") >/tmp/datum-connect-ui.log 2>&1 &
fi

cat <<EOF

✅ Setup complete.

Next steps (UI):
1) In the GUI, create a TCP proxy for 127.0.0.1:${ORIGIN_PORT}
2) Open https://localhost:${TUNNEL_PORT}

Logs:
  dns-dev:   /tmp/datum-connect-dns-dev.log
  serve:     /tmp/datum-connect-serve.log
  gateway:   /tmp/datum-connect-gateway.log
  origin:    /tmp/datum-connect-origin.log
  tunnel:    /tmp/datum-connect-tunnel-dev.log
  ui:        /tmp/datum-connect-ui.log

Press Ctrl+C to stop all services.
EOF

wait
