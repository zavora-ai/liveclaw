# ADK Utilization Matrix

Date: 2026-02-19
Owner: LiveClaw core team
Status: Active

## Purpose

Track ADK-Rust crate/API usage for each roadmap milestone and explicitly document any custom implementation delta.

## Rules

1. Every milestone entry must map features to ADK crates/APIs.
2. If custom code exists where ADK capability is available, provide justification and migration plan.
3. This file is updated at each sprint close.

## Milestone Mapping

| Milestone | Capability Slice | ADK Crates/APIs | LiveClaw Custom Delta | Justification | Target to Reduce Delta |
|---|---|---|---|---|---|
| M0 | Baseline quality and CI | adk-* dependency graph, build/test integration | CI resolution and workspace glue | Repository/CI setup specifics | Sprint 0 close |
| M1 | Voice E2E data path | `adk-realtime`, `adk-runner`, `adk-session` | Gateway transport and session routing | Product-specific gateway protocol | Sprint 2 close |
| M2 | Session auth and isolation | `adk-auth`, `adk-session`, `adk-runner` | Pairing/token protocol and ownership model | LiveClaw transport-level auth flow | Sprint 2 close |
| M3 | Tool + graph execution | `adk-tool`, `adk-auth`, `adk-graph`, `adk-plugin` | Tool loader/wrappers, graph node plumbing | Deployment-specific tools and policy | Sprint 4 close |
| M4 | Memory/artifacts/resilience | `adk-memory`, `adk-artifact`, `adk-runner`, `adk-realtime` | Memory adapter + artifact routing + reconnect glue | Bridge runtime context to gateway/client | Sprint 6 close |
| M5 | Runtime/provider/security parity | `adk-core`, `adk-runner`, `adk-auth`, `adk-telemetry` | Runtime adapter wrappers and diagnostics | Environment-specific runtime modes | Sprint 8 close |
| M6 | Channels/ops release candidate | `adk-runner`, `adk-plugin`, `adk-tool`, `adk-telemetry` | Channel adapters + ops integration | Product-specific channel surfaces | Sprint 11 close |

## Sprint Close Checklist (ADK Compliance)

1. List changed modules and mapped ADK crates/APIs.
2. Confirm no duplicate subsystem reimplementation.
3. Record new custom delta and reduction target.
4. Link demo evidence for ADK usage path.

## Open ADK Gap Log

| Gap ID | Area | Current Blocker | Proposed Approach | Owner | Due |
|---|---|---|---|---|---|
| ADK-GAP-001 | Voice session wiring | CLOSED in Sprint 1: Session audio now forwards through `adk-realtime::RealtimeRunner`, with callback outputs tagged by true session IDs | Maintain this path as the only audio pipeline; add session-ownership hardening in M2 | Core Runtime | M1 |
| ADK-GAP-002 | Graph tools node | CLOSED in Sprint 4: graph tools node now drains pending calls into deterministic state updates, with supervised interrupts covered by tests | Keep tool-loop behavior on `adk-graph` nodes and wire full runtime orchestration incrementally | Core Runtime | M4 |
| ADK-GAP-003 | Plugin parity | CLOSED in Sprint 4: config-gated plugin assembly and per-session rate-limit plugin path implemented | Keep lifecycle hook wiring on `adk-plugin::PluginManager`; avoid bespoke plugin execution paths | Platform | M4 |
| ADK-GAP-004 | Runner lifecycle parity | Active gateway lifecycle currently remains on direct `adk-realtime::RealtimeRunner` orchestration; reconnect/compaction policy is implemented in adapter layer | Migrate active session lifecycle and compaction triggers onto `adk-runner::Runner` path while preserving gateway protocol behavior | Core Runtime | Sprint 7 |
