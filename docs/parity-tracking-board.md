# LiveClaw Parity Tracking Board

Date: 2026-02-19
Status scale: `NOT_STARTED`, `IN_PROGRESS`, `AT_RISK`, `BLOCKED`, `DONE`

## Milestone Board

| Milestone | Status | Owner | Target Date | Demo Script | Acceptance Checks |
|---|---|---|---|---|---|
| M0 Baseline Recovery | DONE | Core Team | 2026-02-27 | `scripts/demo/m0_baseline.sh` | Quality gate green; CI ADK path valid; parity docs present; P0/P1 register maintained |
| M1 Voice E2E | IN_PROGRESS | Core Team | 2026-03-13 | `scripts/demo/m1_voice_e2e.sh` | SessionAudio reaches live runtime; session IDs propagate end-to-end |
| M2 Secure Multi-Session | IN_PROGRESS | Core Team | 2026-03-27 | `scripts/demo/m2_secure_sessions.sh` | Token auth path implemented; per-session ownership enforced |
| M3 Tool and Graph Execution | IN_PROGRESS | Core Team | 2026-04-24 | `scripts/demo/m3_tools_graph.sh` | Non-empty toolset; RBAC enforced; graph tools node executes |
| M4 Memory, Artifacts, Resilience | NOT_STARTED | Core Team | 2026-05-22 | `scripts/demo/m4_memory_artifacts.sh` | Persistent recall across restarts; artifact persistence; reconnect behavior validated |
| M5 ZeroClaw Track Parity | NOT_STARTED | Core Team | 2026-06-19 | `scripts/demo/m5_runtime_security.sh` | Runtime modes validated; provider flexibility; security hardening gates pass |
| M6 OpenClaw Track Parity + RC | NOT_STARTED | Core Team | 2026-07-31 | `scripts/demo/m6_release_flow.sh` | Priority channels work; ops surfaces validated; release candidate checklist passes |

## Sprint 0 Checklist

| Item | Status | Evidence |
|---|---|---|
| Fix clippy failure(s) | DONE | `scripts/run_quality_gate.sh` clippy step now passes |
| Deterministic quality gate script | DONE | `scripts/run_quality_gate.sh` with logs in `output/adk-quality/` |
| CI ADK sibling strategy | DONE | `.github/workflows/ci.yml` uses `zavora-ai/adk-rust` and sibling symlink |
| Demo harness skeleton | DONE | `scripts/demo/run_all.sh` + `m0` through `m6` scripts |
| Reusable browser WS operator client | DONE | `scripts/ws_client.sh` serves `tools/ws-client/index.html` for pair/auth/session/audio validation |
| Parity board with M0-M6 acceptance | DONE | This document |
| P0/P1 defects tracked with owners | DONE | `docs/m0-defect-register.md` |

## Sprint 1 Checklist

| Item | Status | Evidence |
|---|---|---|
| SessionAudio wired to active runtime session | DONE | `liveclaw-app/src/main.rs` `RunnerAdapter::send_audio` now forwards to `adk_realtime::RealtimeRunner::send_audio` |
| Session IDs propagated from runtime callbacks | DONE | `liveclaw-app/src/main.rs` `GatewayEventForwarder` tags outputs with owning session ID |
| Audio ACK protocol semantics corrected | DONE | `liveclaw-gateway/src/server.rs` returns `GatewayResponse::AudioAccepted` |
| Sprint 1 demo script exercises data-plane checks | DONE | `scripts/demo/m1_voice_e2e.sh` targeted tests |
| One-session voice loop integration coverage | DONE | `liveclaw-gateway/src/server.rs` + `liveclaw-app/src/main.rs` tests for audio forward + session tag propagation |

## Sprint 2 Checklist (In Progress)

| Item | Status | Evidence |
|---|---|---|
| Add token authenticate/resume message path | DONE | `liveclaw-gateway/src/protocol.rs` + `liveclaw-gateway/src/server.rs` `Authenticate` and `Authenticated` |
| Enforce per-session ownership for audio/terminate | DONE | `liveclaw-gateway/src/server.rs` `authorize_session_access` |
| Add cross-session misuse tests | DONE | `test_session_audio_rejects_unowned_session`, `test_terminate_rejects_unowned_session` |
| Add reconnect with token control test | DONE | `test_token_auth_can_resume_session_control` |
| Sprint 2 demo script for secure multi-session checks | DONE | `scripts/demo/m2_secure_sessions.sh` |

## Sprint 3 Checklist (In Progress)

| Item | Status | Evidence |
|---|---|---|
| Replace placeholder tool loader with baseline ADK toolset | DONE | `liveclaw-app/src/tools.rs` `build_baseline_tools()` |
| Wire tools into realtime runtime with ADK interfaces | DONE | `liveclaw-app/src/main.rs` converts `Arc<dyn adk_core::Tool>` into realtime tool definitions + handlers |
| Enforce RBAC on tool execution path | DONE | `liveclaw-app/src/main.rs` session-specific `AuthMiddleware::with_audit` |
| Add explicit tool execution metrics (count/failures/duration) | DONE | `liveclaw-app/src/main.rs` `ToolExecutionMetrics` and structured logs |
| Add M3 demo checks | DONE | `scripts/demo/m3_tools_graph.sh` |

## Sprint 4 Checklist (In Progress)

| Item | Status | Evidence |
|---|---|---|
| Implement graph tools-node state transition behavior | DONE | `liveclaw-app/src/graph.rs` `execute_pending_tool_calls()` drains `pending_tool_calls` into `tool_results` |
| Keep deterministic conditional routing for tool loop | DONE | `liveclaw-app/src/graph.rs` `has_pending_tool_calls()` used by `conditional_edge` |
| Validate supervised interrupt-before flow on tools node | DONE | `liveclaw-app/src/graph.rs` tests `supervised_tools_node_interrupts_before_execution` and `non_supervised_tools_node_executes_pending_calls` |
| Respect plugin toggles for PII redaction and memory autosave | DONE | `liveclaw-app/src/plugins.rs` `PluginRuntimeConfig` + conditional plugin assembly in `build_plugin_manager()` |
| Add rate-limiting plugin path in PluginManager | DONE | `liveclaw-app/src/plugins.rs` `build_rate_limit_plugin()` with per-session enforcement |
| Add Sprint 4 demo checks | DONE | `scripts/demo/s4_graph_plugin_completeness.sh` |

## Notes

1. Update this file at each sprint close with status transitions and links to commit SHAs.
2. Mark a milestone `DONE` only when demo script passes and quality gate is green.
3. For gateway-facing milestones, include browser client evidence from `scripts/ws_client.sh` in sprint close notes.
