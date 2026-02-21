# LiveClaw vs OpenClaw vs ZeroClaw Functional Parity Comparison

Date: 2026-02-21  
Scope: onboarding, setup, run flows, plus 20 common user use cases  
Audience: product, engineering, and release planning

## Method and Evidence

This comparison is evidence-based and uses the following sources as of 2026-02-21:

- LiveClaw local docs: `README.md`, `docs/parity-tracking-board.md`, `docs/final-parity-closeout-summary.md`
- OpenClaw primary sources:
  - `https://github.com/openclaw/openclaw`
  - `https://raw.githubusercontent.com/openclaw/openclaw/main/README.md`
  - `https://raw.githubusercontent.com/openclaw/openclaw/main/docs/start/getting-started.md`
  - `https://raw.githubusercontent.com/openclaw/openclaw/main/docs/start/setup.md`
  - `https://raw.githubusercontent.com/openclaw/openclaw/main/docs/start/onboarding.md`
  - `https://raw.githubusercontent.com/openclaw/openclaw/main/docs/start/wizard.md`
- ZeroClaw primary sources:
  - `https://github.com/zeroclaw-labs/zeroclaw`
  - `https://raw.githubusercontent.com/zeroclaw-labs/zeroclaw/main/README.md`
  - `https://raw.githubusercontent.com/zeroclaw-labs/zeroclaw/main/docs/getting-started/README.md`
  - `https://raw.githubusercontent.com/zeroclaw-labs/zeroclaw/main/docs/commands-reference.md`
  - `https://raw.githubusercontent.com/zeroclaw-labs/zeroclaw/main/docs/channels-reference.md`
  - `https://raw.githubusercontent.com/zeroclaw-labs/zeroclaw/main/docs/operations-runbook.md`

Status legend:

- `Full`: capability is clearly documented and demonstrable in current scope
- `Partial`: capability exists but is limited in UX, coverage, or evidence depth
- `Gap`: not currently provided in practical user flow
- `Unknown`: not sufficiently evidenced in the pulled primary sources

## 1) Onboarding, Setup, and Run Comparison

| Dimension | LiveClaw | OpenClaw | ZeroClaw |
|---|---|---|---|
| Primary install path | `cargo` workspace setup with ADK-Rust sibling + `dev/setup.sh` | install script or npm/pnpm global install (`openclaw`) | Homebrew/bootstrap/cargo install paths |
| First-time onboarding UX | Guided onboarding via `liveclaw-app onboard` (interactive/non-interactive) with config validation + optional service install/start | Strong guided onboarding (`openclaw onboard`, macOS app onboarding) | Guided onboarding (`zeroclaw onboard --interactive` and quick onboarding flags) |
| First run command | `cargo run -p liveclaw-app -- liveclaw.toml` | `openclaw onboard --install-daemon`, then `openclaw dashboard`/`openclaw gateway` | `zeroclaw onboard ...`, then `zeroclaw daemon` or `zeroclaw gateway` |
| Foreground runtime mode | Full | Full | Full |
| Background service mode | Full (`service install/start/stop/restart/status/uninstall` with macOS launchd and Linux systemd user units) | Full (`--install-daemon`) | Full (`zeroclaw service ...`) |
| Health and diagnostics | Full (`GetGatewayHealth`, `GetDiagnostics`, `--doctor`) | Full (`openclaw health`, doctor docs) | Full (`zeroclaw status`, `doctor`, `channel doctor`) |
| Client/operator UI | Browser WS client (`scripts/ws_client.sh`) focused on gateway protocol operations | Full dashboard/control UI + app surfaces | Primarily CLI/operator docs; UI not primary |
| Auth bootstrapping | Pairing code -> token -> `Authenticate` | Wizard/app onboarding + pairing/allowlists | Pairing and bearer token model documented |
| Runtime/provider flexibility | Full (`runtime.kind=native/docker`, provider profiles, doctor validation) | Partial (broad model/provider support; runtime model less central) | Full (native/docker runtime and provider abstraction documented) |
| ADK-Rust core alignment | Full (explicit ADK-first architecture in docs and milestones) | N/A (TypeScript ecosystem) | N/A (Rust trait-based, not ADK-Rust) |

## 2) 20 Common User Use Cases (Parity Matrix)

| # | Common User Use Case | LiveClaw | OpenClaw | ZeroClaw | Evidence Notes |
|---|---|---|---|---|---|
| 1 | Install from scratch on macOS/Linux | Partial | Full | Full | LiveClaw currently assumes Rust + ADK sibling workflow; OpenClaw and ZeroClaw provide clearer turnkey install paths |
| 2 | Guided onboarding wizard | Full | Full | Full | LiveClaw now provides `onboard` with interactive/non-interactive flows, config generation, validation, and optional service wiring |
| 3 | Start long-running background service | Full | Full | Full | LiveClaw now provides first-class `service` command group for lifecycle operations |
| 4 | Check health/status quickly | Full | Full | Full | All three have operator-visible health/diagnostic flow |
| 5 | Pair and authenticate a client securely | Full | Full | Full | LiveClaw: `Pair` + token + `Authenticate`; OpenClaw/ZeroClaw have pairing + allowlist/token models |
| 6 | Create and manage isolated sessions | Full | Full | Partial | LiveClaw and OpenClaw have explicit session-oriented control planes; ZeroClaw is strong but session API details are less explicit in pulled docs |
| 7 | Send text prompt and get response | Full | Full | Full | LiveClaw `SessionPrompt`; OpenClaw `agent`/dashboard; ZeroClaw `agent` |
| 8 | Stream live microphone audio | Full | Full | Unknown | LiveClaw has browser mic streaming and protocol support; OpenClaw voicewake/talk mode is documented |
| 9 | Send prerecorded audio for processing | Full | Full | Unknown | LiveClaw WS client supports audio upload + PCM conversion + playback |
| 10 | Prompt-driven tool invocation | Full | Full | Full | LiveClaw demonstrates prompt -> tool path (`read_workspace_file`); OpenClaw/ZeroClaw tool systems documented |
| 11 | Direct tool invocation via API/protocol | Full | Partial | Partial | LiveClaw has explicit `SessionToolCall`; others emphasize agent-mediated tool use |
| 12 | Inspect graph/tool execution trace | Full | Unknown | Partial | LiveClaw returns graph trace in `SessionToolResult.graph`; ZeroClaw documents LangGraph companion path |
| 13 | Enforce RBAC/policy at tool execution time | Full | Partial | Full | LiveClaw explicit role model + audit; ZeroClaw security policy is explicit; OpenClaw policy model is present but less RBAC-centric in pulled docs |
| 14 | Enforce workspace file boundaries | Full | Partial | Full | LiveClaw and ZeroClaw explicitly document workspace/path controls; OpenClaw sandbox policy is present but differs by session mode |
| 15 | Persist memory across restarts | Full | Full | Full | LiveClaw M4 acceptance explicitly validates persistence; ZeroClaw and OpenClaw document memory/session persistence |
| 16 | Persist/retrieve artifacts/transcripts | Full | Partial | Partial | LiveClaw M4 includes artifact service and probes; others have session/log persistence but less explicit artifact API evidence |
| 17 | Integrate Telegram/Slack/Webhook ingress | Full | Full | Full | LiveClaw implements adapter and route tests for Telegram/Slack/Webhook |
| 18 | Retrieve outbound channel messages programmatically | Full | Unknown | Partial | LiveClaw has `GetChannelOutbound` + HTTP poll; this was not clearly surfaced in OpenClaw docs pulled |
| 19 | Schedule recurring jobs/automations | Full | Full | Full | LiveClaw channel jobs; OpenClaw cron/webhook automation; ZeroClaw cron command suite |
| 20 | Survive provider/runtime interruptions with policy-based retries | Full | Partial | Full | LiveClaw M4 resilience evidence includes reconnect/backoff tests; ZeroClaw runtime/ops posture is explicit |

## 3) LiveClaw Parity Summary

### High-confidence strengths

1. Voice-first WS control plane with session ownership, pairing/token auth, and protocol-level diagnostics.
2. Tool and graph execution with explicit traceability and RBAC guardrails.
3. Memory, artifacts, and resilience are demonstrated with milestone evidence and runnable scripts.
4. Runtime/provider security hardening is explicit (`native`/`docker`, allowlist enforcement, public bind safety checks).
5. Browser client now includes guided bootstrap/ops workflows plus reusable saved connection profiles.

### Main parity gaps versus OpenClaw + ZeroClaw

1. User-facing product surfaces gap: OpenClaw has richer end-user app/dashboard experience out of the box.
2. Channel breadth gap: LiveClaw currently focuses on priority channels and protocol surfaces rather than broad channel ecosystem breadth.

## 4) Recommended Closure Order (Pragmatic)

1. `P0` UX unification: promote WS client as first-class control UI with guided tasks and saved environment profiles.
2. `P1` Channel breadth expansion based on actual user demand sequence.
3. `P1` Operator ergonomics follow-up: add service log-tail and diagnostics shortcuts under `service` command group.

## 5) Bottom Line

LiveClaw is strong on core runtime correctness (voice, tools, memory, resilience, security) and now closes the onboarding + service-management CLI gaps. The largest remaining parity deltas are richer end-user UX surfaces and broader channel ecosystem breadth.
