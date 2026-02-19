#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "${SCRIPT_DIR}/common.sh"

demo_header "M2 Secure Multi-Session"

(
  cd "${ROOT_DIR}"

  # Token authentication from existing pairing token
  cargo test -p liveclaw-gateway server::tests::test_authenticate_with_valid_token -- --exact
  cargo test -p liveclaw-gateway server::tests::test_authenticate_rejects_invalid_token -- --exact

  # Reconnect/resume authorization with token
  cargo test -p liveclaw-gateway server::tests::test_token_auth_can_resume_session_control -- --exact

  # Cross-session ownership enforcement
  cargo test -p liveclaw-gateway server::tests::test_session_audio_rejects_unowned_session -- --exact
  cargo test -p liveclaw-gateway server::tests::test_terminate_rejects_unowned_session -- --exact
)

demo_pass "Token auth and per-session ownership checks passed"
