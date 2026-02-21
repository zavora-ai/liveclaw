#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "${SCRIPT_DIR}/common.sh"

demo_header "M6 Release Candidate Flow"

(
  cd "${ROOT_DIR}"

  cargo test -p liveclaw-gateway --lib protocol::tests::test_gateway_message_get_gateway_health -- --exact
  cargo test -p liveclaw-gateway --lib protocol::tests::test_gateway_response_gateway_health -- --exact
  cargo test -p liveclaw-gateway --lib protocol::tests::test_gateway_message_priority_probe -- --exact
  cargo test -p liveclaw-gateway --lib protocol::tests::test_gateway_message_channel_inbound -- --exact
  cargo test -p liveclaw-gateway --lib protocol::tests::test_gateway_response_priority_notice -- --exact
  cargo test -p liveclaw-gateway --lib protocol::tests::test_gateway_response_priority_probe_accepted -- --exact
  cargo test -p liveclaw-gateway --lib protocol::tests::test_gateway_response_channel_routed -- --exact
  cargo test -p liveclaw-gateway --lib server::tests::test_get_gateway_health_works_without_auth -- --exact
  cargo test -p liveclaw-gateway --lib server::tests::test_priority_probe_requires_auth -- --exact
  cargo test -p liveclaw-gateway --lib server::tests::test_priority_probe_enqueues_priority_and_standard_messages -- --exact
  cargo test -p liveclaw-gateway --lib server::tests::test_channel_inbound_requires_auth -- --exact
  cargo test -p liveclaw-gateway --lib server::tests::test_channel_inbound_reuses_session_for_same_route_key -- --exact
  cargo test -p liveclaw-gateway --lib server::tests::test_channel_inbound_isolates_channel_and_account_routes -- --exact
  cargo test -p liveclaw-gateway --lib server::tests::test_terminate_session_cleans_channel_routes -- --exact
  cargo test -p liveclaw-gateway --lib server::tests::test_channel_webhook_http_requires_auth_token -- --exact
  cargo test -p liveclaw-gateway --lib server::tests::test_channel_webhook_http_routes_with_bearer_token -- --exact
  cargo test -p liveclaw-gateway --lib server::tests::test_channel_slack_http_routes_message_event -- --exact
  cargo test -p liveclaw-gateway --lib server::tests::test_channel_slack_http_url_verification_returns_challenge -- --exact
  cargo test -p liveclaw-gateway --lib server::tests::test_channel_telegram_http_routes_message -- --exact
  cargo test -p liveclaw-gateway --lib protocol::tests::test_supported_protocol_message_lists_include_diagnostics_path -- --exact
)

demo_pass "Gateway WS/HTTP health, priority control-plane, and channel-routing checks passed"
