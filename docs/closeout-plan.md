# LiveClaw Project Closeout Plan

Date: 2026-02-21
Status: Phase 5 complete
Owner: Core Team

## Objective

Close the remaining parity and release-candidate scope so the project can be declared complete with auditable evidence.

## Completion Criteria

1. `docs/parity-tracking-board.md` milestones are closed with evidence.
2. `docs/m0-defect-register.md` has no open P0/P1 defects.
3. `docs/adk-utilization-matrix.md` open ADK gaps are closed or explicitly waived.
4. `scripts/run_quality_gate.sh` passes.
5. `scripts/demo/run_all.sh` passes.
6. Live M1 voice roundtrip (`scripts/demo/m1_voice_e2e_live.sh`) passes against a provider-backed runtime.
7. M6 release-candidate scope is delivered: channels, operations surfaces, and RC checklist artifacts.

## Baseline Evidence Snapshot (2026-02-21)

1. Quality gate passed: `scripts/run_quality_gate.sh`.
2. Milestone demo harness passed: `scripts/demo/run_all.sh`.
3. Live provider-backed voice roundtrip passed:
   `LIVECLAW_E2E_USE_EXISTING_GATEWAY=never LIVECLAW_E2E_CONFIG=<temp_config> LIVECLAW_E2E_WS_URL=ws://127.0.0.1:8520/ws scripts/demo/m1_voice_e2e_live.sh`.
4. Current status: M1-M6 acceptance checks are evidenced; RC sign-off record is published.

## Remaining Blockers

1. None.

## Phase Plan and Sprints

## Phase 1: Truth Alignment (2026-02-21 to 2026-02-24)

1. Align milestone and defect docs to current verified evidence.
2. Publish closeout backlog with owners and target sprints.
3. Exit criteria:
   - Tracking docs reflect current state without stale open items.
   - Remaining blockers are explicitly mapped to execution phases.

## Phase 2: Runtime and Channel Parity (Sprint 13, 2026-03-09 to 2026-03-20)

1. Implement Telegram, Slack, and Webhook channel adapters.
2. Add channel routing isolation and channel smoke tests.
4. Exit criteria:
   - Channel demo script passes with auth/routing evidence.

## Phase 3: Tooling and Automation Surfaces (Sprint 14, 2026-03-23 to 2026-04-03)

1. Deliver browser, HTTP, and cron tool surfaces with policy controls.
2. Add background scheduling and webhook-trigger start path.
   - Status: HTTP + interval-based channel scheduling delivered, including webhook-supervised trigger path and cron/operator execution evidence.
3. Add operator visibility for active sessions and jobs.
4. Exit criteria:
   - Tooling/automation demo script passes with policy checks.

## Phase 4: RC Hardening (Sprint 15, 2026-04-06 to 2026-04-17)

1. Add concurrent-session load tests and publish limits.
   - Status: delivered via `test_concurrent_create_session_burst_updates_health_and_ownership` and `docs/rc-operational-limits.md`.
2. Create release runbook, deployment guide, and rollback playbook.
   - Status: delivered (`docs/release-runbook.md`, `docs/deployment-guide.md`, `docs/rollback-playbook.md`).
3. Finalize smoke suite and CI sign-off path.
   - Status: delivered via `scripts/demo/m6_rc_signoff.sh` and `docs/rc-signoff-record.md`.
4. Exit criteria:
   - RC checklist is complete and reproducible.

## Phase 5: Closure and Tagging (Sprint 16, 2026-04-20 to 2026-04-24)

1. Mark milestone board complete.
   - Status: delivered (`docs/parity-tracking-board.md` marks M0-M6 `DONE`).
2. Publish final parity closeout summary and ADK delta report.
   - Status: delivered (`docs/final-parity-closeout-summary.md`, `docs/adk-delta-report.md`).
3. Tag release-candidate milestone and archive closeout evidence.
   - Status: delivered (`milestone/M6` tag published).
4. Exit criteria:
   - Project completion criteria are fully satisfied.

## Execution Policy

1. Commit at least once per completed vertical slice.
2. Do not leave uncommitted functional work at day end.
3. Each closeout commit must include:
   - Behavior change.
   - Tests or explicit test rationale.
   - Updated evidence in docs/demo scripts when applicable.

## Kickoff Log

1. 2026-02-21: created closeout plan document.
2. 2026-02-21: began Phase 1 by reconciling parity, defect, and ADK tracking docs with current verification evidence.
3. 2026-02-21: completed ADK lifecycle closeout (`ADK-GAP-004`) by wiring transcript lifecycle and compaction snapshots through `adk-runner::Runner` in the active gateway path.
4. 2026-02-21: completed docker runtime closeout (`ADK-GAP-005`) by adding containerized realtime worker mode (`--runtime-worker`) and docker command/event bridge runtime (`DockerRuntimeBridgeRuntime`) in active session creation flow.
5. 2026-02-21: started channel parity closeout slice by adding authenticated `ChannelInbound` routing with principal/channel/account/user isolation, browser WS client controls, and M6 demo/test coverage.
6. 2026-02-21: completed HTTP ingress adapter slice for Webhook/Slack/Telegram in gateway with token-authenticated routing into `ChannelInbound` and route-level smoke tests.
7. 2026-02-21: completed outbound delivery contract slice by adding channel-scoped outbound queueing for final transcripts and retrieval via WS (`GetChannelOutbound`) and HTTP (`/channels/outbound/poll`).
8. 2026-02-21: completed channel scheduler slice by adding principal-scoped `CreateChannelJob` / `ListChannelJobs` / `CancelChannelJob`, `active_channel_jobs` health visibility, and browser WS client controls with M6 demo coverage.
9. 2026-02-21: completed HTTP channel-job surface (`/channels/jobs/create`, `/channels/jobs/list`, `/channels/jobs/cancel`) with token-authenticated policy checks and release-flow test coverage.
10. 2026-02-21: completed webhook-supervised trigger path (`/channels/webhook/supervised-action`) that forces `SessionConfig.role=supervised`, executes `SessionToolCall` through the graph path, and returns action results via HTTP.
11. 2026-02-21: closed cron/operator policy evidence by adding deterministic channel-job tick execution tests (`test_channel_job_tick_routes_text_for_owner_principal`, `test_channel_job_tick_honors_create_response_flag`) and wiring them into `scripts/demo/m6_release_flow.sh`.
12. 2026-02-21: started Phase 4 RC hardening by adding concurrent session burst coverage (`test_concurrent_create_session_burst_updates_health_and_ownership`) and wiring it into `scripts/demo/m6_release_flow.sh`.
13. 2026-02-21: published RC operation artifacts (`docs/release-runbook.md`, `docs/deployment-guide.md`, `docs/rollback-playbook.md`).
14. 2026-02-21: published RC operational limits (`docs/rc-operational-limits.md`) with deterministic burst evidence and re-verification command path.
15. 2026-02-21: completed smoke-suite sign-off workflow via `scripts/demo/m6_rc_signoff.sh` and published `docs/rc-signoff-record.md`.
16. 2026-02-21: published Phase 5 closure artifacts (`docs/final-parity-closeout-summary.md`, `docs/adk-delta-report.md`) and promoted milestone board to M0-M6 `DONE`.
17. 2026-02-21: published release-candidate milestone tag `milestone/M6` and completed Phase 5 closure.
18. 2026-02-21: closed onboarding/service parity deltas by adding `onboard` and `service` CLI command groups in `liveclaw-app`, plus README + parity-board evidence updates.
19. 2026-02-21: closed operator UX parity slice by adding saved connection profiles and guided bootstrap/ops/README-summary workflows to the reusable browser WS client, with README + parity-board evidence updates.
20. 2026-02-21: closed service-ergonomics parity slice by adding `service logs` and `service doctor` shortcuts in `liveclaw-app`, with parser tests and README/parity documentation updates.
21. 2026-02-21: improved install-path parity by adding `scripts/bootstrap_local.sh` for one-command local setup + non-interactive onboarding (including optional `--clone-adk` auto-bootstrap), and aligned README/parity tracking docs.
22. 2026-02-21: expanded channel breadth by adding Discord ingress (`/channels/discord/events`) with payload translation, route normalization support, gateway tests, demo coverage, and WS client/docs updates.
23. 2026-02-21: reduced user-surface parity gap by adding a simple chat lane to the browser WS client (session prompt + transcript history UX) and aligning README/parity-board reporting.
