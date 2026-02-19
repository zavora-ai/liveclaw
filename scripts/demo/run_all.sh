#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

scripts=(
  "m0_baseline.sh"
  "m1_voice_e2e.sh"
  "m2_secure_sessions.sh"
  "m3_tools_graph.sh"
  "m4_memory_artifacts.sh"
  "m5_runtime_security.sh"
  "m6_release_flow.sh"
)

failed=0

for script in "${scripts[@]}"; do
  echo
  if ! "${SCRIPT_DIR}/${script}"; then
    failed=$((failed + 1))
  fi
done

echo
if [[ ${failed} -ne 0 ]]; then
  echo "Demo run failed: ${failed} script(s) failed"
  exit 1
fi

echo "Demo run complete: all scripts succeeded (pass or pending)"
