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
  cargo test -p liveclaw-gateway --lib protocol::tests::test_gateway_message_get_channel_outbound -- --exact
  cargo test -p liveclaw-gateway --lib protocol::tests::test_gateway_message_create_channel_job -- --exact
  cargo test -p liveclaw-gateway --lib protocol::tests::test_gateway_message_cancel_channel_job -- --exact
  cargo test -p liveclaw-gateway --lib protocol::tests::test_gateway_message_list_channel_jobs -- --exact
  cargo test -p liveclaw-gateway --lib protocol::tests::test_gateway_response_priority_notice -- --exact
  cargo test -p liveclaw-gateway --lib protocol::tests::test_gateway_response_priority_probe_accepted -- --exact
  cargo test -p liveclaw-gateway --lib protocol::tests::test_gateway_response_channel_routed -- --exact
  cargo test -p liveclaw-gateway --lib protocol::tests::test_gateway_response_channel_outbound_batch -- --exact
  cargo test -p liveclaw-gateway --lib protocol::tests::test_gateway_response_channel_job_created -- --exact
  cargo test -p liveclaw-gateway --lib protocol::tests::test_gateway_response_channel_job_canceled -- --exact
  cargo test -p liveclaw-gateway --lib protocol::tests::test_gateway_response_channel_jobs -- --exact
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
  cargo test -p liveclaw-gateway --lib server::tests::test_get_channel_outbound_requires_auth -- --exact
  cargo test -p liveclaw-gateway --lib server::tests::test_get_channel_outbound_returns_queued_final_transcript_items -- --exact
  cargo test -p liveclaw-gateway --lib server::tests::test_channel_outbound_poll_http_requires_auth_token -- --exact
  cargo test -p liveclaw-gateway --lib server::tests::test_channel_outbound_poll_http_returns_and_drains_items -- --exact
  cargo test -p liveclaw-gateway --lib server::tests::test_create_channel_job_requires_auth -- --exact
  cargo test -p liveclaw-gateway --lib server::tests::test_list_channel_jobs_requires_auth -- --exact
  cargo test -p liveclaw-gateway --lib server::tests::test_cancel_channel_job_requires_auth -- --exact
  cargo test -p liveclaw-gateway --lib server::tests::test_create_channel_job_rejects_zero_interval -- --exact
  cargo test -p liveclaw-gateway --lib server::tests::test_channel_job_lifecycle_create_list_cancel -- --exact
  cargo test -p liveclaw-gateway --lib server::tests::test_cancel_channel_job_rejects_non_owner -- --exact
  cargo test -p liveclaw-gateway --lib protocol::tests::test_supported_protocol_message_lists_include_diagnostics_path -- --exact
)

demo_pass "Gateway WS/HTTP health, priority control-plane, channel routing/outbound, and channel-job scheduler checks passed"
