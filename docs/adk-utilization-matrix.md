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
| ADK-GAP-002 | Graph tools node | Tool node is stubbed | Implement `adk-graph` node execution over pending tool calls | TBD | M3 |
| ADK-GAP-003 | Plugin parity | Config toggles/rate-limit plugin incomplete | Complete `adk-plugin` hook mapping and config gating | TBD | M4 |
