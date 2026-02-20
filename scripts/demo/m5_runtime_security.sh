#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "${SCRIPT_DIR}/common.sh"

demo_header "M5 Runtime and Security Parity"

(
  cd "${ROOT_DIR}"

  # Runtime mode parsing rejects invalid kinds.
  cargo test -p liveclaw-app --lib config::tests::test_runtime_kind_rejects_unknown_value -- --exact

  # Provider profile selection and validation for OpenAI-compatible endpoints.
  cargo test -p liveclaw-app --bin liveclaw-app tests::test_resolve_provider_selection_openai_compatible_profile -- --exact
  cargo test -p liveclaw-app --bin liveclaw-app tests::test_validate_runtime_and_provider_rejects_missing_compat_base_url -- --exact
  cargo test -p liveclaw-app --bin liveclaw-app tests::test_validate_runtime_and_provider_rejects_non_ws_base_url -- --exact

  # Provider env diagnostic matrix and doctor command output.
  ./scripts/provider_env_matrix.sh
  doctor_out="$(mktemp)"
  cargo run -p liveclaw-app -- --doctor dev/liveclaw.dev.toml >"${doctor_out}"
  grep -q "LiveClaw doctor report" "${doctor_out}"
  rm -f "${doctor_out}"
)

demo_pass "Runtime mode and provider profile diagnostics checks passed"
