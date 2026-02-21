# LiveClaw Parity Tracking Board

Date: 2026-02-21
Status scale: `NOT_STARTED`, `IN_PROGRESS`, `AT_RISK`, `BLOCKED`, `DONE`

## Milestone Board

| Milestone | Status | Owner | Target Date | Demo Script | Acceptance Checks |
|---|---|---|---|---|---|
| M0 Baseline Recovery | DONE | Core Team | 2026-02-27 | `scripts/demo/m0_baseline.sh` | Quality gate green; CI ADK path valid; parity docs present; P0/P1 register maintained |
| M1 Voice E2E | DONE | Core Team | 2026-03-13 | `scripts/demo/m1_voice_e2e.sh` + `scripts/demo/m1_voice_e2e_live.sh` | SessionAudio reaches live runtime; provider-backed transcript/audio roundtrip validated; session IDs propagate end-to-end |
| M2 Secure Multi-Session | DONE | Core Team | 2026-03-27 | `scripts/demo/m2_secure_sessions.sh` | Token auth path implemented; per-session ownership enforced |
| M3 Tool and Graph Execution | DONE | Core Team | 2026-04-24 | `scripts/demo/m3_tools_graph.sh` | Non-empty toolset; RBAC enforced; graph tools node executes; `SessionToolCall` emits graph trace |
| M4 Memory, Artifacts, Resilience | DONE | Core Team | 2026-05-22 | `scripts/demo/m4_memory_artifacts.sh` | Persistent recall across restarts; artifact persistence; reconnect behavior validated |
| M5 ZeroClaw Track Parity | DONE | Core Team | 2026-06-19 | `scripts/demo/m5_runtime_security.sh` | Runtime modes validated; provider flexibility; security hardening gates pass |
| M6 OpenClaw Track Parity + RC | IN_PROGRESS | Core Team | 2026-07-31 | `scripts/demo/m6_release_flow.sh` | Priority channels work; ops surfaces validated; release candidate checklist passes |

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

## Sprint 2 Checklist (DONE)

| Item | Status | Evidence |
|---|---|---|
| Add token authenticate/resume message path | DONE | `liveclaw-gateway/src/protocol.rs` + `liveclaw-gateway/src/server.rs` `Authenticate` and `Authenticated` |
| Enforce per-session ownership for audio/terminate | DONE | `liveclaw-gateway/src/server.rs` `authorize_session_access` |
| Add cross-session misuse tests | DONE | `test_session_audio_rejects_unowned_session`, `test_terminate_rejects_unowned_session` |
| Add reconnect with token control test | DONE | `test_token_auth_can_resume_session_control` |
| Sprint 2 demo script for secure multi-session checks | DONE | `scripts/demo/m2_secure_sessions.sh` |

## Sprint 3 Checklist (DONE)

| Item | Status | Evidence |
|---|---|---|
| Replace placeholder tool loader with baseline ADK toolset | DONE | `liveclaw-app/src/tools.rs` `build_baseline_tools()` |
| Wire tools into realtime runtime with ADK interfaces | DONE | `liveclaw-app/src/main.rs` converts `Arc<dyn adk_core::Tool>` into realtime tool definitions + handlers |
| Enforce RBAC on tool execution path | DONE | `liveclaw-app/src/main.rs` session-specific `AuthMiddleware::with_audit` |
| Add explicit tool execution metrics (count/failures/duration) | DONE | `liveclaw-app/src/main.rs` `ToolExecutionMetrics` and structured logs |
| Add client-triggered tool invocation message | DONE | `liveclaw-gateway/src/protocol.rs` + `liveclaw-gateway/src/server.rs` `SessionToolCall` and `SessionToolResult` |
| Execute tool calls through graph path and return execution trace | DONE | `liveclaw-app/src/main.rs` `execute_session_tool_call_graph()` + graph trace tests |
| Keep browser WS client aligned for tool + graph validation | DONE | `tools/ws-client/index.html` Tool + Graph panel and `SessionToolResult` trace rendering |
| Add M3 demo checks | DONE | `scripts/demo/m3_tools_graph.sh` |

## Sprint 4 Checklist (DONE)

| Item | Status | Evidence |
|---|---|---|
| Implement graph tools-node state transition behavior | DONE | `liveclaw-app/src/graph.rs` `execute_pending_tool_calls()` drains `pending_tool_calls` into `tool_results` |
| Keep deterministic conditional routing for tool loop | DONE | `liveclaw-app/src/graph.rs` `has_pending_tool_calls()` used by `conditional_edge` |
| Validate supervised interrupt-before flow on tools node | DONE | `liveclaw-app/src/graph.rs` tests `supervised_tools_node_interrupts_before_execution` and `non_supervised_tools_node_executes_pending_calls` |
| Respect plugin toggles for PII redaction and memory autosave | DONE | `liveclaw-app/src/plugins.rs` `PluginRuntimeConfig` + conditional plugin assembly in `build_plugin_manager()` |
| Add rate-limiting plugin path in PluginManager | DONE | `liveclaw-app/src/plugins.rs` `build_rate_limit_plugin()` with per-session enforcement |
| Add Sprint 4 demo checks | DONE | `scripts/demo/s4_graph_plugin_completeness.sh` |

## Sprint 5 Checklist (DONE)

| Item | Status | Evidence |
|---|---|---|
| Add durable file-backed memory backend option | DONE | `liveclaw-app/src/storage.rs` `FileMemoryService` + `build_memory_service()` (`file` and `file:/path`) |
| Persist transcript memories across callback events and on session close | DONE | `liveclaw-app/src/main.rs` `GatewayEventForwarder::persist_transcript_memory` + terminate summary persistence |
| Add file-backed artifact service via ADK artifact contracts | DONE | `liveclaw-app/src/storage.rs` `FileArtifactService` implements `adk_artifact::ArtifactService` |
| Persist transcript and audio artifacts from realtime callbacks | DONE | `liveclaw-app/src/main.rs` `persist_transcript_artifact` + `persist_audio_artifact` |
| Add M4 demo checks for memory/artifact persistence | DONE | `scripts/demo/m4_memory_artifacts.sh` |

## Sprint 6 Checklist (DONE)

| Item | Status | Evidence |
|---|---|---|
| Implement reconnect/backoff policy for interrupted provider sessions | DONE | `liveclaw-app/src/main.rs` `ReconnectPolicy`, `reconnect_with_backoff()`, `spawn_runtime_loop()` |
| Enable config-driven memory compaction behavior | DONE | `liveclaw-app/src/main.rs` `MemoryCompactionPolicy` + `compact_memory_entries()` applied in transcript persistence path |
| Add reliability tests for reconnect recovery and retry exhaustion | DONE | `tests::test_runtime_loop_recovers_from_interrupted_provider_session`, `tests::test_runtime_loop_stops_when_reconnect_budget_exhausted` |
| Add Sprint 6 checks to M4 demo script | DONE | `scripts/demo/m4_memory_artifacts.sh` now runs compaction + reconnect tests |
| Keep lifecycle and compaction execution in `adk-runner` orchestration path | DONE | `liveclaw-app/src/main.rs` `SessionLifecycleManager` now initializes session lifecycle through `adk-runner::Runner`, ingests transcript events via runner invocations, and snapshots compaction-aware memory entries; covered by `test_session_lifecycle_manager_records_transcript_entries` and `test_session_lifecycle_manager_reports_compaction_delta` |

## Sprint 7 Checklist (DONE)

| Item | Status | Evidence |
|---|---|---|
| Add explicit runtime adapter modes (`native`, `docker`) with fail-fast parsing | DONE | `liveclaw-app/src/config.rs` `RuntimeKind` enum and `config::tests::test_runtime_kind_rejects_unknown_value` |
| Add provider profile matrix with OpenAI-compatible endpoint support | DONE | `liveclaw-app/src/config.rs` `ProvidersConfig`; `liveclaw-app/src/main.rs` `resolve_provider_selection` with `OpenAIRealtimeModel::with_base_url` |
| Add runtime/provider configuration validation and doctor diagnostics | DONE | `liveclaw-app/src/main.rs` `validate_runtime_and_provider` + `--doctor` command path |
| Add provider env matrix helper for operators | DONE | `scripts/provider_env_matrix.sh` |
| Expose live diagnostics via gateway for WS client validation | DONE | `liveclaw-gateway/src/protocol.rs` `GetDiagnostics` / `Diagnostics`; `liveclaw-gateway/src/server.rs` handler and tests |
| Keep browser client aligned with protocol via server-advertised message capabilities | DONE | `RuntimeDiagnostics.supported_client_messages` consumed by `tools/ws-client/index.html` template sync |
| Add Sprint 7 checks in M5 demo script | DONE | `scripts/demo/m5_runtime_security.sh` |

## Sprint 8 Checklist (DONE)

| Item | Status | Evidence |
|---|---|---|
| Add workspace-scoped file-read tool for supervised/full sessions | DONE | `liveclaw-app/src/tools.rs` `read_workspace_file` + policy/path resolution tests |
| Enforce deny-by-default principal allowlist mode at session creation | DONE | `liveclaw-app/src/main.rs` `RunnerAdapter::create_session` allowlist gate + `test_create_session_denied_when_principal_not_allowlisted` |
| Block public gateway bind unless explicit security override is set | DONE | `liveclaw-app/src/main.rs` `validate_runtime_and_provider` public bind checks + override warning test |
| Expose security posture in diagnostics and keep browser client aligned | DONE | `liveclaw-gateway/src/protocol.rs` security diagnostics fields + `tools/ws-client/index.html` security badge updates |
| Extend M5 parity script with Sprint 8 security checks | DONE | `scripts/demo/m5_runtime_security.sh` includes public bind and allowlist tests |

## Sprint 9 Checklist (DONE)

| Item | Status | Evidence |
|---|---|---|
| Add unauthenticated gateway health WS command for operator checks | DONE | `liveclaw-gateway/src/protocol.rs` `GetGatewayHealth` + `GatewayHealth`; `liveclaw-gateway/src/server.rs` health handler |
| Keep browser WS client aligned with gateway health surfaces | DONE | `tools/ws-client/index.html` adds Gateway Health action and `gw` badge |
| Replace M6 placeholder demo with executable ops-surface checks | DONE | `scripts/demo/m6_release_flow.sh` now runs targeted protocol/server tests |

## Sprint 10 Checklist (DONE)

| Item | Status | Evidence |
|---|---|---|
| Add priority control-plane probe path to validate channel precedence | DONE | `liveclaw-gateway/src/protocol.rs` `PriorityProbe`, `PriorityNotice`, `PriorityProbeAccepted`; `liveclaw-gateway/src/server.rs` probe handler |
| Track priority channel bindings in gateway health snapshot | DONE | `GatewayHealth.active_priority_bindings` in `liveclaw-gateway/src/protocol.rs` and `liveclaw-gateway/src/server.rs` |
| Keep browser WS client aligned with priority probe/notice flow | DONE | `tools/ws-client/index.html` adds Priority Probe action, template, and `prio` badge |
| Extend M6 release-flow demo for priority-path verification | DONE | `scripts/demo/m6_release_flow.sh` includes priority probe tests |

## Sprint 11 Checklist (DONE)

| Item | Status | Evidence |
|---|---|---|
| Add provider-backed live voice roundtrip script for true M1 E2E validation | DONE | `scripts/demo/m1_voice_e2e_live.sh` (gateway startup/reuse, WS session, audio stream, session-scoped audio/transcript assertions) |
| Add deterministic audio generation helper for E2E harness | DONE | `scripts/generate_e2e_pcm.sh` uses OpenAI speech API (`format=pcm`) |
| Align M1 milestone acceptance criteria to require live transcript/audio evidence | DONE | Milestone board row now references both `m1_voice_e2e.sh` and `m1_voice_e2e_live.sh` |
| Harden live harness for operator environments with running gateways | DONE | `scripts/demo/m1_voice_e2e_live.sh` supports `LIVECLAW_E2E_USE_EXISTING_GATEWAY`, token/pair-code auth overrides, and richer failure diagnostics |
| Add manual turn-control voice protocol for deterministic prerecorded-audio flows | DONE | `SessionAudioCommit` / `SessionResponseCreate` / `SessionResponseInterrupt` in `liveclaw-gateway/src/protocol.rs` + `liveclaw-gateway/src/server.rs` + `liveclaw-app/src/main.rs` runner wiring |
| Keep reusable browser WS client aligned with full voice E2E path | DONE | `tools/ws-client/index.html` adds mic streaming, upload-to-PCM conversion, `AudioOutput` playback, and session transcript pane |

## Sprint 12 Checklist (DONE)

| Item | Status | Evidence |
|---|---|---|
| Implement containerized runtime worker mode for `runtime.kind=docker` | DONE | `liveclaw-app/src/main.rs` adds `--runtime-worker` command path and worker command/event protocol (`DockerWorkerCommand` / `DockerWorkerMessage`) |
| Route docker sessions through active runtime bridge | DONE | `liveclaw-app/src/main.rs` `DockerRuntimeBridgeRuntime` now selected in `RunnerAdapter::create_session` when `runtime.kind=docker` |
| Remove compatibility-mode warning and surface real docker diagnostics | DONE | `validate_runtime_and_provider()` now reports `runtime.docker_image` note without compatibility fallback warning |
| Add docker bridge tests and demo coverage | DONE | `test_build_docker_runtime_command_includes_worker_mode_and_env`, `test_validate_runtime_and_provider_docker_reports_runtime_image_note`, and `scripts/demo/m5_runtime_security.sh` |

## Sprint 13 Checklist (DONE)

| Item | Status | Evidence |
|---|---|---|
| Add gateway protocol surfaces for channel ingress routing | DONE | `liveclaw-gateway/src/protocol.rs` adds `ChannelInbound` and `ChannelRouted` plus supported-type coverage tests |
| Add principal-scoped route isolation by channel/account/external user | DONE | `liveclaw-gateway/src/server.rs` `channel_routes` map with `ChannelRouteKey` and routing tests |
| Keep browser WS client aligned for channel bridge verification | DONE | `tools/ws-client/index.html` adds Channel Bridge panel and `ChannelInbound` raw template |
| Extend M6 demo checks for channel-routing behavior | DONE | `scripts/demo/m6_release_flow.sh` adds protocol + server channel-routing tests |
| Implement external Telegram/Slack/Webhook adapter services | DONE | `liveclaw-gateway/src/server.rs` adds HTTP ingress endpoints (`/channels/webhook`, `/channels/slack/events`, `/channels/telegram/update`) with token auth and payload translation tests |
| Expose channel-adapter verification in M6 demo flow | DONE | `scripts/demo/m6_release_flow.sh` runs HTTP adapter route tests (`test_channel_webhook_http_*`, `test_channel_slack_http_*`, `test_channel_telegram_http_*`) |
| Implement outbound channel delivery contract | DONE | `liveclaw-gateway/src/protocol.rs` adds `GetChannelOutbound`/`ChannelOutboundBatch`; `liveclaw-gateway/src/server.rs` queues final transcripts per channel route and exposes WS + HTTP outbound poll (`/channels/outbound/poll`) |
| Keep browser WS client aligned for outbound contract validation | DONE | `tools/ws-client/index.html` adds outbound poll controls and `ChannelOutboundBatch` activity view |

## Sprint 14 Checklist (DONE)

| Item | Status | Evidence |
|---|---|---|
| Add gateway protocol surfaces for channel job lifecycle | DONE | `liveclaw-gateway/src/protocol.rs` adds `CreateChannelJob` / `CancelChannelJob` / `ListChannelJobs` and `ChannelJobCreated` / `ChannelJobCanceled` / `ChannelJobs` |
| Implement principal-scoped background channel scheduler | DONE | `liveclaw-gateway/src/server.rs` adds `channel_jobs` runtime map, interval job worker, ownership checks, and health integration |
| Add scheduler lifecycle tests (auth, validation, owner isolation) | DONE | `liveclaw-gateway/src/server.rs` tests `test_create_channel_job_requires_auth`, `test_channel_job_lifecycle_create_list_cancel`, `test_cancel_channel_job_rejects_non_owner`, plus interval validation checks |
| Keep browser WS client aligned for channel job operations | DONE | `tools/ws-client/index.html` adds Channel Job controls, activity pane, response rendering, and raw templates for create/list/cancel |
| Add HTTP scheduler surface for channel jobs with token-auth policy | DONE | `liveclaw-gateway/src/server.rs` adds `/channels/jobs/create`, `/channels/jobs/list`, `/channels/jobs/cancel` with auth + ownership enforcement and HTTP tests |
| Add webhook-triggered supervised action entrypoint | DONE | `liveclaw-gateway/src/server.rs` adds `/channels/webhook/supervised-action` with enforced `SessionConfig.role=supervised` + graph-enabled tool-call path and HTTP tests |
| Extend M6 release-flow checks for scheduler surfaces | DONE | `scripts/demo/m6_release_flow.sh` includes protocol + server tests for channel job messages/responses and lifecycle |
| Deliver remaining tooling/automation closeout surfaces (policy-controlled browser/http/cron) | DONE | `liveclaw-gateway/src/server.rs` adds cron tick execution coverage (`test_channel_job_tick_routes_text_for_owner_principal`, `test_channel_job_tick_honors_create_response_flag`) and `scripts/demo/m6_release_flow.sh` validates the checks end-to-end |

## Sprint 15 Checklist (IN_PROGRESS)

| Item | Status | Evidence |
|---|---|---|
| Add concurrent-session load test evidence and publish operational limits | IN_PROGRESS | `liveclaw-gateway/src/server.rs` adds `test_concurrent_create_session_burst_updates_health_and_ownership` and `scripts/demo/m6_release_flow.sh` now exercises the burst path |
| Create release runbook, deployment guide, and rollback playbook | DONE | `docs/release-runbook.md`, `docs/deployment-guide.md`, `docs/rollback-playbook.md` |
| Finalize smoke suite and CI sign-off path | IN_PROGRESS | `scripts/run_quality_gate.sh` + `scripts/demo/run_all.sh` as sign-off gates; release RC sign-off record still open |

## Closeout Phase 1 Kickoff (2026-02-21)

| Item | Status | Evidence |
|---|---|---|
| Re-run deterministic quality gate | DONE | `scripts/run_quality_gate.sh` passed (`check`, `test`, `clippy`, `fmt`) |
| Re-run milestone demo harness | DONE | `scripts/demo/run_all.sh` passed end-to-end |
| Validate provider-backed live voice roundtrip | DONE | `scripts/demo/m1_voice_e2e_live.sh` passed in isolated mode (`LIVECLAW_E2E_USE_EXISTING_GATEWAY=never`) |
| Promote milestone statuses where acceptance is fully evidenced | DONE | M1-M5 moved to `DONE`; M6 remains `IN_PROGRESS` |
| Capture remaining project-complete blockers and execution plan | DONE | `docs/closeout-plan.md` |

## Notes

1. Update this file at each sprint close with status transitions and links to commit SHAs.
2. Mark a milestone `DONE` only when demo script passes and quality gate is green.
3. For gateway-facing milestones, include browser client evidence from `scripts/ws_client.sh` in sprint close notes.
