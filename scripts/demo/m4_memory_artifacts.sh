#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "${SCRIPT_DIR}/common.sh"

demo_header "M4 Memory, Artifacts, Resilience"

(
  cd "${ROOT_DIR}"

  # Durable memory backend survives process restart.
  cargo test -p liveclaw-app --lib storage::tests::file_memory_persists_across_restarts -- --exact

  # Artifact persistence and retrieval survive service restart.
  cargo test -p liveclaw-app --lib storage::tests::file_artifact_round_trip_persists_across_restarts -- --exact

  # Runtime callback path writes transcript/audio artifacts and transcript memory.
  cargo test -p liveclaw-app --bin liveclaw-app tests::test_gateway_event_forwarder_persists_memory_and_artifacts -- --exact

  # Config-driven transcript compaction keeps memory bounded.
  cargo test -p liveclaw-app --bin liveclaw-app tests::test_compact_memory_entries_reduces_history_when_threshold_exceeded -- --exact

  # Reconnect/backoff policy recovers from interrupted provider sessions.
  cargo test -p liveclaw-app --bin liveclaw-app tests::test_runtime_loop_recovers_from_interrupted_provider_session -- --exact
  cargo test -p liveclaw-app --bin liveclaw-app tests::test_runtime_loop_stops_when_reconnect_budget_exhausted -- --exact
)

demo_pass "Memory, artifact, compaction, and reconnect checks passed"
