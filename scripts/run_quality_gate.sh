#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT_DIR="${ROOT_DIR}/output/adk-quality"
mkdir -p "${OUT_DIR}"

overall_status=0

run_step() {
  local name="$1"
  shift
  local log_file="${OUT_DIR}/${name}.log"

  echo "[gate] ${name}"
  set +e
  "$@" 2>&1 | tee "${log_file}"
  local step_status=${PIPESTATUS[0]}
  set -e
  if [[ ${step_status} -ne 0 ]]; then
    echo "[gate] ${name}: FAIL (see ${log_file})"
    overall_status=1
  else
    echo "[gate] ${name}: PASS"
  fi
  echo
}

cd "${ROOT_DIR}"

run_step check cargo check --workspace --all-features
run_step test cargo test --workspace --all-features
run_step clippy cargo clippy --workspace --all-targets --all-features -- -D warnings
run_step fmt cargo fmt -p liveclaw-gateway -p liveclaw-app -- --check

if [[ ${overall_status} -ne 0 ]]; then
  echo "[gate] QUALITY GATE FAILED"
  exit 1
fi

echo "[gate] QUALITY GATE PASSED"
