# M0 Defect Register (P0/P1)

Date: 2026-02-19
Scope: Baseline defects identified during parity/readiness review

## Open Defects

| ID | Severity | Area | Defect | Owner | Target Milestone | Status |
|---|---|---|---|---|---|---|
| LC-001 | P0 | Voice data plane | `SessionAudio` path is effectively dropped by runner adapter no-op | Core Runtime | M1 | CLOSED |
| LC-002 | P0 | Session routing | Agent output session IDs are hard-coded instead of propagated per session | Core Runtime | M1 | CLOSED |
| LC-003 | P1 | Session security | Missing per-session ownership checks for terminate/audio actions | Security | M2 | CLOSED |
| LC-004 | P1 | Authentication | Pairing/token lifecycle lacks full token-authenticate flow semantics | Security | M2 | CLOSED |
| LC-005 | P1 | Protocol correctness | `SessionAudio` success uses `SessionCreated` response semantics | Gateway | M1 | CLOSED |
| LC-006 | P1 | Tooling | Tool loader is placeholder (empty) and does not provide production-capable baseline tools | Agent Runtime | M3 | CLOSED |
| LC-007 | P1 | Graph execution | Graph tools node is stubbed and does not execute pending tool calls | Agent Runtime | M3 | CLOSED |
| LC-008 | P1 | Plugin completeness | Plugin config toggles not fully enforced and rate-limiting plugin path incomplete | Platform | M4 | CLOSED |
| LC-009 | P1 | Compaction | Compaction config exists but runtime compaction is disabled | Platform | M4 | OPEN |

## Ownership Notes

1. Owners are role-level placeholders until individual assignment at sprint planning.
2. No defect may move to `CLOSED` without:
   - linked commit SHA
   - test coverage evidence
   - demo evidence in milestone script

## Resolution Notes (Sprint 1)

1. `LC-001` closed by wiring `SessionAudio` to per-session `adk_realtime::RealtimeRunner` instances in `liveclaw-app/src/main.rs`.
2. `LC-002` closed by replacing hard-coded `"default"` tagging with session-bound `GatewayEventForwarder` callback routing in `liveclaw-app/src/main.rs`.
3. `LC-005` closed by returning `GatewayResponse::AudioAccepted` for `SessionAudio` success in `liveclaw-gateway/src/server.rs`.
4. `LC-003` closed by enforcing stable principal ownership checks in `liveclaw-gateway/src/server.rs` before audio/terminate operations.
5. `LC-004` closed by introducing token-based `Authenticate` flow with resume control tests in `liveclaw-gateway/src/protocol.rs` and `liveclaw-gateway/src/server.rs`.
6. `LC-006` closed by implementing baseline `adk-tool::FunctionTool` catalog and session-wired RBAC execution path in `liveclaw-app/src/tools.rs` and `liveclaw-app/src/main.rs`.

## Resolution Notes (Sprint 4)

1. `LC-007` closed by implementing deterministic tools-node execution state updates in `liveclaw-app/src/graph.rs` (`execute_pending_tool_calls`) and covering supervised/non-supervised execution paths in graph tests.
2. `LC-008` closed by adding config-driven plugin assembly (`PluginRuntimeConfig`) and per-session rate-limiting plugin enforcement in `liveclaw-app/src/plugins.rs`, with toggle and behavior tests.
