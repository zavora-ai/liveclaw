# LiveClaw Parity Tracking Board

Date: 2026-02-19
Status scale: `NOT_STARTED`, `IN_PROGRESS`, `AT_RISK`, `BLOCKED`, `DONE`

## Milestone Board

| Milestone | Status | Owner | Target Date | Demo Script | Acceptance Checks |
|---|---|---|---|---|---|
| M0 Baseline Recovery | IN_PROGRESS | Core Team | 2026-02-27 | `scripts/demo/m0_baseline.sh` | Quality gate green; CI ADK path valid; parity docs present |
| M1 Voice E2E | NOT_STARTED | Core Team | 2026-03-13 | `scripts/demo/m1_voice_e2e.sh` | SessionAudio reaches live runtime; session IDs propagate end-to-end |
| M2 Secure Multi-Session | NOT_STARTED | Core Team | 2026-03-27 | `scripts/demo/m2_secure_sessions.sh` | Token auth path implemented; per-session ownership enforced |
| M3 Tool and Graph Execution | NOT_STARTED | Core Team | 2026-04-24 | `scripts/demo/m3_tools_graph.sh` | Non-empty toolset; RBAC enforced; graph tools node executes |
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
| Parity board with M0-M6 acceptance | DONE | This document |

## Notes

1. Update this file at each sprint close with status transitions and links to commit SHAs.
2. Mark a milestone `DONE` only when demo script passes and quality gate is green.
