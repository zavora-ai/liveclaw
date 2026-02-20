#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CLIENT_DIR="${ROOT_DIR}/tools/ws-client"
HOST="${1:-127.0.0.1}"
PORT="${2:-18080}"
URL="http://${HOST}:${PORT}/index.html"
OPEN_BROWSER="${OPEN_BROWSER:-1}"

if [[ ! -d "${CLIENT_DIR}" ]]; then
  echo "Client directory not found: ${CLIENT_DIR}" >&2
  exit 1
fi

if ! command -v python3 >/dev/null 2>&1; then
  echo "python3 is required to serve the browser client." >&2
  exit 1
fi

echo "Serving LiveClaw WS client at ${URL}"
if [[ "${OPEN_BROWSER}" == "1" ]] && command -v open >/dev/null 2>&1; then
  open "${URL}" >/dev/null 2>&1 || true
fi

cd "${CLIENT_DIR}"
python3 -m http.server "${PORT}" --bind "${HOST}"
