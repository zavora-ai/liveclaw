#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "${SCRIPT_DIR}/common.sh"

demo_header "M1 Voice E2E"

(
  cd "${ROOT_DIR}"

  # Session-tagged callback forwarding from adk-realtime runtime -> gateway output channels
  cargo test -p liveclaw-app --bin liveclaw-app tests::test_gateway_event_forwarder_tags_audio_and_transcript -- --exact

  # Gateway accepts SessionAudio and routes to runner handle with correct ACK semantics
  cargo test -p liveclaw-gateway server::tests::test_session_audio_forwards_to_runner -- --exact
  cargo test -p liveclaw-gateway protocol::tests::test_gateway_response_audio_accepted -- --exact
)

demo_pass "SessionAudio path and session ID propagation checks passed"
