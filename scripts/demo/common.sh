#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

demo_header() {
  local name="$1"
  echo "== ${name} =="
}

demo_pass() {
  local msg="$1"
  echo "PASS: ${msg}"
}

demo_pending() {
  local msg="$1"
  echo "PENDING: ${msg}"
}

require_file() {
  local path="$1"
  if [[ ! -f "${ROOT_DIR}/${path}" ]]; then
    echo "FAIL: required file missing: ${path}"
    return 1
  fi
}
