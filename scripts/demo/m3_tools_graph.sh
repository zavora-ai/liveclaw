#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "${SCRIPT_DIR}/common.sh"

demo_header "M3 Tool and Graph Execution"

(
  cd "${ROOT_DIR}"

  # Baseline ADK FunctionTool catalog exists with explicit schemas.
  cargo test -p liveclaw-app --lib tools::tests::baseline_tools_are_non_empty_with_explicit_schemas -- --exact

  # Role enforcement on tool execution path (readonly denied, full allowed).
  cargo test -p liveclaw-app --bin liveclaw-app tests::test_tool_handler_denies_readonly_role -- --exact
  cargo test -p liveclaw-app --bin liveclaw-app tests::test_tool_handler_executes_for_full_role_and_tracks_metrics -- --exact
)

demo_pass "Baseline tool execution and RBAC checks passed"
