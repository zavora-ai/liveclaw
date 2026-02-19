# LiveClaw Parity Roadmap and Project Plan

Date: 2026-02-19
Planning horizon: 24 weeks
Cadence: 2-week sprints (with Sprint 0 as a 1-week stabilization sprint)

## Program Goals

1. Achieve functional parity on core runtime capabilities that matter for real users:
   - Voice session reliability
   - Secure multi-session control plane
   - Safe tool execution with policy enforcement
   - Persistent memory and recoverability
2. Build targeted parity with ZeroClaw on portability/security/runtime flexibility.
3. Build targeted parity with OpenClaw on channel and operations surfaces most relevant to LiveClaw's scope.
4. Deliver demonstrable functionality at every milestone with test evidence.

## Parity Definition (What "Done" Means)

1. Feature parity does not mean cloning every upstream feature.
2. Parity means equivalent user outcomes for prioritized workflows:
   - "Can I run secure voice sessions end-to-end?"
   - "Can I operate this reliably in real environments?"
   - "Can I safely expose and scale across channels/tools?"
3. Every parity claim must include:
   - A runnable demo script
   - Automated tests (unit + integration)
   - A benchmark or reliability metric

## Non-Negotiable Delivery Rules

1. No sprint closes without a working demo.
2. No milestone closes with red CI.
3. No partial work left uncommitted at end of workday.
4. Every merged slice must be releasable behind config or feature flags if incomplete.

## Milestone Ladder (Demonstrable Outcomes)

| Milestone | Target Date | Demonstrable Outcome |
|---|---|---|
| M0 Baseline Recovery | 2026-02-27 | Quality gate green, roadmap and demo harness in repo, known P0/P1 defects tracked with owners |
| M1 Voice E2E | 2026-03-13 | Real SessionAudio reaches live agent; audio + transcript return on correct session IDs |
| M2 Secure Multi-Session | 2026-03-27 | Token-based re-auth + per-session ownership checks + clean protocol semantics |
| M3 Tool and Graph Execution | 2026-04-24 | Non-empty toolset executes with RBAC, supervised interrupts, and deterministic graph loop |
| M4 Memory, Artifacts, Resilience | 2026-05-22 | Persistent memory recall across sessions, artifacts saved, provider reconnect behavior validated |
| M5 ZeroClaw Track Parity | 2026-06-19 | Runtime/provider/memory/security hardening comparable for target scenarios |
| M6 OpenClaw Track Parity + RC | 2026-07-31 | Priority channels + ops surfaces delivered, release candidate with runbook and smoke suite |

## Phase Plan

## Phase 0: Stabilization and Baseline (Sprint 0)
Timeline: 2026-02-23 to 2026-02-27
Objective: Restore engineering control and remove gate blockers.

Scope:
1. Fix clippy failure and formatting drift.
2. Establish CI truth path with ADK dependency strategy that actually resolves in CI.
3. Add sprint demo harness skeleton (`scripts/demo/`).
4. Create parity tracking board (M0-M6, each with acceptance checks).

Demo gate:
1. One-command quality gate run passes locally and in CI.
2. Demo harness command returns all milestone demos as "pending" or "pass" with no broken scripts.

Exit criteria:
1. `cargo check`, `cargo test`, `cargo clippy -D warnings`, `cargo fmt --check` all green.
2. CI workflow executes without manual patching.

## Phase 1: Core Voice Runtime Parity (Sprints 1-2)
Timeline: 2026-03-02 to 2026-03-27
Objective: Deliver reliable and secure core voice sessions.

Sprint 1 (2026-03-02 to 2026-03-13): Voice E2E Data Plane
1. Wire `SessionAudio` to actual runner/agent session.
2. Remove hard-coded `"default"` session tagging and propagate true session IDs.
3. Correct protocol semantics for audio acknowledgements.
4. Add integration tests for one-session voice loop.

Demo gate:
1. Start gateway.
2. Pair.
3. Create session.
4. Stream audio.
5. Receive audio/transcript updates on same session.

Sprint 2 (2026-03-16 to 2026-03-27): Session Security and Auth
1. Add token-based authenticate/resume path (not only pairing code).
2. Enforce per-session ownership checks for audio/terminate.
3. Harden auth errors and audit events.
4. Add tests for cross-session misuse attempts.

Demo gate:
1. Two clients, two sessions.
2. Client A cannot control Client B session.
3. Reconnect with token restores authorized control.

## Phase 2: ADK Functional Parity (Sprints 3-5)
Timeline: 2026-03-30 to 2026-05-22
Objective: Turn stubs into working orchestration, tools, memory, and resilience.

Sprint 3 (2026-03-30 to 2026-04-10): Tool Execution Reality
1. Replace placeholder `load_tools()` with actual baseline toolset.
2. Ensure AuthMiddleware-protected tools execute end-to-end.
3. Add explicit tool execution metrics (duration, count, failures).

Demo gate:
1. Voice prompt triggers tool call.
2. Authorized role succeeds.
3. ReadOnly role is denied and audited.

Sprint 4 (2026-04-13 to 2026-04-24): Graph and Plugin Completeness
1. Implement actual graph `tools` node behavior.
2. Respect plugin config toggles (`enable_pii_redaction`, `enable_memory_autosave`).
3. Add missing rate-limiting plugin behavior as specified.
4. Validate supervised interrupt-before flow.

Demo gate:
1. Supervised session pauses before tool execution.
2. Human approval resumes tool execution.
3. PII redaction can be toggled on/off by config.

Sprint 5 (2026-04-27 to 2026-05-08): Memory + Artifacts
1. Implement durable memory backend option (starting with SQLite or equivalent).
2. Store and recall session summaries meaningfully (not only session ID placeholder).
3. Persist transcript artifacts and optional audio artifacts.

Demo gate:
1. Session 1 stores memory facts.
2. Session 2 recalls facts after process restart.
3. Transcript artifact retrievable by session ID.

Sprint 6 (2026-05-11 to 2026-05-22): Resilience + Compaction
1. Implement reconnect/backoff policy for provider drops.
2. Enable and validate compaction strategy from config.
3. Add reliability tests for interrupted provider sessions.

Demo gate:
1. Kill provider connection mid-session.
2. Session recovers automatically within configured retry budget.
3. Long session compaction keeps context bounded.

## Phase 3: ZeroClaw Parity Track (Sprints 7-8)
Timeline: 2026-05-25 to 2026-06-19
Objective: Match ZeroClaw strengths on runtime flexibility, security posture, and portability.

Sprint 7 (2026-05-25 to 2026-06-05): Runtime and Provider Flexibility
1. Add explicit runtime adapter modes (`native`, `docker`), fail-fast on invalid runtime kind.
2. Add provider profile matrix with OpenAI-compatible custom endpoint support.
3. Add configuration validation and doctor diagnostics.

Demo gate:
1. Same scenario runs in native and docker runtime modes.
2. Provider switch works without code change.
3. Invalid runtime/provider config fails with actionable error.

Sprint 8 (2026-06-08 to 2026-06-19): Security Hardening Layer
1. Enforce workspace scoping and forbidden path policy for file-capable tools.
2. Add deny-by-default channel/user allowlist pattern for inbound integrations.
3. Add tunnel/public-bind safety constraints for gateway exposure.

Demo gate:
1. Attempted workspace escape is blocked and audited.
2. Unauthorized inbound identity is denied with operator approval path.
3. Public bind without explicit safe config is refused.

## Phase 4: OpenClaw Parity Track + Release (Sprints 9-11)
Timeline: 2026-06-22 to 2026-07-31
Objective: Deliver high-value channel and ops parity for production usage.

Sprint 9 (2026-06-22 to 2026-07-03): Priority Channels
1. Implement first channel set: Telegram + Slack + Webhook bridge.
2. Add routing isolation by channel/account/session.
3. Add integration smoke tests for inbound/outbound message flows.

Demo gate:
1. Same agent runtime handles inbound Telegram and Slack.
2. Channel policies enforce routing and authorization correctly.

Sprint 10 (2026-07-06 to 2026-07-17): Tools and Automation Surfaces
1. Add browser/http/cron baseline tool surfaces with policy control.
2. Add background task scheduling and webhook-trigger entrypoint.
3. Add operator visibility for active sessions/jobs.

Demo gate:
1. Scheduled task executes and reports result.
2. Browser or HTTP tool runs within policy constraints.
3. Webhook trigger starts supervised agent action.

Sprint 11 (2026-07-20 to 2026-07-31): Release Candidate
1. Hardening pass on security, retries, and telemetry.
2. Performance and load test for concurrent sessions.
3. Ship release runbook, deployment doc, and rollback playbook.

Demo gate:
1. Full end-to-end demo: pair/auth, multi-channel input, tool execution, memory recall, artifact retrieval.
2. RC checklist sign-off with green CI and smoke suite.

## Sprint-Level Structure (Applied Every Sprint)

1. Sprint planning:
   - Finalize 3-5 vertical stories max.
   - Each story has one demo condition and one test condition.
2. Mid-sprint checkpoint:
   - Demonstrate partial vertical slices by day 5.
3. Sprint close:
   - Run quality gate.
   - Run sprint demo script.
   - Publish sprint notes and known gaps.

## Commit Progress Always (Operating Policy)

1. Branching:
   - Use branch naming `codex/<phase>-<sprint>-<topic>`.
2. Commit frequency:
   - Commit at least once per completed vertical slice.
   - No more than 4 working hours without a commit.
3. Commit quality:
   - Every commit must compile.
   - Every commit includes tests for changed behavior or explicit note why absent.
4. Commit format:
   - `feat(scope): ...`, `fix(scope): ...`, `test(scope): ...`, `docs(scope): ...`, `chore(scope): ...`.
5. Milestone tagging:
   - Tag milestone completions: `milestone/M0` ... `milestone/M6`.
6. PR merge rule:
   - No merge without demo evidence and quality gate pass.

## Demonstration Framework

1. Add scripts:
   - `scripts/demo/m0_baseline.sh`
   - `scripts/demo/m1_voice_e2e.sh`
   - `scripts/demo/m2_secure_sessions.sh`
   - `scripts/demo/m3_tools_graph.sh`
   - `scripts/demo/m4_memory_artifacts.sh`
   - `scripts/demo/m5_runtime_security.sh`
   - `scripts/demo/m6_release_flow.sh`
2. Each script must:
   - Exit non-zero on failure.
   - Print a concise pass/fail summary.
   - Be runnable in CI where feasible.

## Metrics and Acceptance Targets

1. Reliability:
   - Session success rate >= 99% on smoke scenarios.
2. Security:
   - Zero known P0/P1 auth/session-isolation findings open at milestone close.
3. Quality:
   - Clippy and fmt always green on main.
4. Performance:
   - Track p50 and p95 latency for first transcript and first audio.
5. Delivery:
   - Milestone demos pass in reproducible environment.

## Risk Register and Mitigations

1. ADK dependency drift risk.
   - Mitigation: pin revisions and run weekly dependency compatibility check.
2. Scope explosion from full OpenClaw feature surface.
   - Mitigation: maintain parity-by-outcome list and defer non-core surfaces to post-M6.
3. Security regressions while adding channels/tools.
   - Mitigation: mandatory threat-model checkpoint each sprint.
4. Integration instability across providers.
   - Mitigation: provider contract tests and fallback profiles.

## Immediate Next Actions (Week 1)

1. Close P0 data-plane defects before any new feature track starts.
2. Make quality gate deterministic and green.
3. Implement M0 demo harness and lock milestone acceptance criteria.
4. Start Sprint 1 only after M0 acceptance is signed off.
