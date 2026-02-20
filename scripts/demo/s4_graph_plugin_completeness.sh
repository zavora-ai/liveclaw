#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "${SCRIPT_DIR}/common.sh"

demo_header "S4 Graph and Plugin Completeness"

(
  cd "${ROOT_DIR}"

  # Graph tools-node state behavior.
  cargo test -p liveclaw-app --lib graph::tests::pending_tool_calls_detected_only_for_non_empty_arrays -- --exact
  cargo test -p liveclaw-app --lib graph::tests::tools_node_drains_pending_calls_into_results -- --exact
  cargo test -p liveclaw-app --lib graph::tests::tools_node_handles_missing_tool_fields -- --exact
  cargo test -p liveclaw-app --lib graph::tests::supervised_tools_node_interrupts_before_execution -- --exact
  cargo test -p liveclaw-app --lib graph::tests::non_supervised_tools_node_executes_pending_calls -- --exact

  # Plugin toggle and rate-limit behavior.
  cargo test -p liveclaw-app --lib plugins::tests::plugin_manager_respects_toggle_flags -- --exact
  cargo test -p liveclaw-app --lib plugins::tests::plugin_manager_can_disable_optional_plugins -- --exact
  cargo test -p liveclaw-app --lib plugins::tests::pii_redaction_toggle_controls_on_event_behavior -- --exact
  cargo test -p liveclaw-app --lib plugins::tests::rate_limit_plugin_blocks_after_limit -- --exact
)

demo_pass "Graph tools-node flow and plugin toggle/rate-limit checks passed"
