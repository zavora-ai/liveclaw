#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "${SCRIPT_DIR}/common.sh"

demo_header "M6 Release Candidate Flow"

(
  cd "${ROOT_DIR}"

  cargo test -p liveclaw-gateway --lib protocol::tests::test_gateway_message_get_gateway_health -- --exact
  cargo test -p liveclaw-gateway --lib protocol::tests::test_gateway_response_gateway_health -- --exact
  cargo test -p liveclaw-gateway --lib server::tests::test_get_gateway_health_works_without_auth -- --exact
  cargo test -p liveclaw-gateway --lib protocol::tests::test_supported_protocol_message_lists_include_diagnostics_path -- --exact
)

demo_pass "Gateway WS health ops surface checks passed"
