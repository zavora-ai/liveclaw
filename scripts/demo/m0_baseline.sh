#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "${SCRIPT_DIR}/common.sh"

demo_header "M0 Baseline Recovery"

require_file "docs/parity-roadmap.md"
require_file "docs/adk-utilization-matrix.md"
require_file "docs/parity-tracking-board.md"
require_file "scripts/run_quality_gate.sh"

"${ROOT_DIR}/scripts/run_quality_gate.sh"

demo_pass "M0 baseline checks are green"
