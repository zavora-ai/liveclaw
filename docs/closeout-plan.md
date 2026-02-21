# LiveClaw Project Closeout Plan

Date: 2026-02-21
Status: Phase 1 started
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
4. Current status: M1-M5 acceptance checks are evidenced; M6 remains in progress.

## Remaining Blockers

1. OpenClaw-track scope not yet implemented end-to-end:
   - Browser/HTTP/Cron baseline tool surfaces with policy controls.
   - Webhook-triggered supervised actions and cron/operator policy evidence (interval scheduling implemented, but closeout evidence is incomplete).
2. Release candidate artifacts:
   - Runbook, deployment guide, rollback playbook.
   - Concurrent-session load test evidence and smoke-suite sign-off.

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
   - Status: interval-based channel scheduling delivered; webhook-supervised trigger policy path remains.
3. Add operator visibility for active sessions and jobs.
4. Exit criteria:
   - Tooling/automation demo script passes with policy checks.

## Phase 4: RC Hardening (Sprint 15, 2026-04-06 to 2026-04-17)

1. Add concurrent-session load tests and publish limits.
2. Create release runbook, deployment guide, and rollback playbook.
3. Finalize smoke suite and CI sign-off path.
4. Exit criteria:
   - RC checklist is complete and reproducible.

## Phase 5: Closure and Tagging (Sprint 16, 2026-04-20 to 2026-04-24)

1. Mark milestone board complete.
2. Publish final parity closeout summary and ADK delta report.
3. Tag release-candidate milestone and archive closeout evidence.
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
