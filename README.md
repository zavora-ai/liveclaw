# LiveClaw

Conceived and designed by [Michaell Faille](https://github.com/mikefaille) and [James Karanja Maina](https://github.com/jkmaina).

A voice-first, real-time agent runtime built in Rust on top of [ADK-Rust](https://github.com/adk-rust). LiveClaw provides a WebSocket gateway for managing bidirectional voice sessions with AI agents, featuring pairing authentication, role-based access control, PII redaction, persistent memory, and plugin-based extensibility.

## Architecture

LiveClaw is organized as a Cargo workspace with two crates:

| Crate | Purpose |
|---|---|
| `liveclaw-gateway` | WebSocket server, pairing authentication, session routing, gateway protocol |
| `liveclaw-app` | Entry point, TOML config, RealtimeAgent/Runner/GraphAgent wiring, memory adapter, plugins, security callbacks |

The runtime delegates heavily to ADK-Rust crates for voice orchestration (`adk-realtime`), session lifecycle (`adk-runner`), event-sourced persistence (`adk-session`), agent memory (`adk-memory`), multi-step orchestration (`adk-graph`), access control (`adk-auth`), plugin hooks (`adk-plugin`), and observability (`adk-telemetry`).

## Features

- Real-time bidirectional voice streaming via `RealtimeAgent` with server-side VAD and barge-in
- WebSocket gateway with JSON protocol for session management
- One-time 6-digit pairing code authentication with brute-force lockout
- Role-based access control (ReadOnly, Supervised, Full) with JSONL audit logging
- PII redaction plugin (email, phone, SSN, credit card, IP)
- Shell injection detection in tool arguments
- Per-session rate limiting on tool executions
- Persistent agent memory across sessions via `MemoryStore`
- Optional `GraphAgent` wrapping for multi-step orchestration with human-in-the-loop
- TOML-based configuration with sensible defaults
- OpenTelemetry integration for tracing and metrics

## Prerequisites

- Rust 1.75+ (2021 edition)
- [ADK-Rust](https://github.com/adk-rust) cloned as a sibling directory (`../adk-rust/`)

## Quick Start

```bash
# One-command bootstrap (setup + non-interactive onboard + doctor)
LIVECLAW_API_KEY=sk-... ./scripts/bootstrap_local.sh \
  --clone-adk https://github.com/zavora-ai/adk-rust.git \
  --provider-profile openai \
  --model gpt-4o-realtime-preview-2024-12-17 \
  --gateway-host 127.0.0.1 \
  --gateway-port 8420 \
  --require-pairing true

# Or run setup + onboarding step-by-step (after cloning adk-rust as ../adk-rust)
./dev/setup.sh
cargo run -p liveclaw-app -- onboard

# Run gateway from config
cargo run -p liveclaw-app -- liveclaw.toml
```

## Onboarding and Service Commands

The `liveclaw-app` binary now exposes first-class onboarding and service lifecycle commands.

```bash
# Help
cargo run -p liveclaw-app -- help

# Validate config without starting the gateway
cargo run -p liveclaw-app -- doctor liveclaw.toml

# Install service from existing config
cargo run -p liveclaw-app -- service install --config liveclaw.toml

# Start/stop/restart/status
cargo run -p liveclaw-app -- service start
cargo run -p liveclaw-app -- service status
cargo run -p liveclaw-app -- service restart
cargo run -p liveclaw-app -- service stop

# Service diagnostics shortcuts
cargo run -p liveclaw-app -- service doctor --config liveclaw.toml
cargo run -p liveclaw-app -- service logs --lines 120
cargo run -p liveclaw-app -- service logs --stream stderr --lines 80
cargo run -p liveclaw-app -- service logs --follow

# Uninstall service
cargo run -p liveclaw-app -- service uninstall
```

Platform notes:
- macOS uses user `launchd` (`~/Library/LaunchAgents/ai.liveclaw.gateway.plist`).
- Linux uses user `systemd` (`~/.config/systemd/user/liveclaw.service`).

## Configuration

LiveClaw loads configuration from a TOML file. See [`dev/liveclaw.dev.toml`](dev/liveclaw.dev.toml) for a complete example with all options.

Key sections:

| Section | Description |
|---|---|
| `[gateway]` | Host, port, pairing toggle |
| `[voice]` | Provider, API key, model, voice, instructions |
| `[security]` | Default role, tool allowlist, rate limit, audit log path, workspace tool boundary, principal allowlist mode, public bind override |
| `[plugin]` | PII redaction, memory auto-save, blocked keywords |
| `[memory]` | Backend type, recall limit |
| `[graph]` | Enable graph orchestration, recursion limit |
| `[compaction]` | Transcript compaction toggle and memory threshold |
| `[artifact]` | Artifact persistence toggle and storage path |
| `[pairing]` | Max attempts, lockout duration |
| `[runtime]` | Runtime mode (`native`/`docker`) and runtime image metadata |
| `[providers]` | Active provider profile and profile-specific endpoint/API settings |
| `[resilience]` | Provider reconnect policy (attempts and backoff) |
| `[telemetry]` | OTLP export toggle |

Missing fields use documented defaults. Unknown fields are ignored.

Provider and runtime diagnostics:

```bash
# Validate runtime/provider config without starting the gateway
cargo run -p liveclaw-app -- --doctor dev/liveclaw.dev.toml

# Show provider env var availability matrix
./scripts/provider_env_matrix.sh
```

Security hardening diagnostics exposed via `GetDiagnostics`:
- `security_workspace_root`
- `security_forbidden_tool_paths`
- `security_deny_by_default_principal_allowlist`
- `security_principal_allowlist_size`
- `security_allow_public_bind`

## Development

```bash
# Check compilation
cargo check

# Run tests
cargo test

# Lint
cargo clippy

# Format
cargo fmt --check
```

## RC Operations Docs

- Runbook: [`docs/release-runbook.md`](docs/release-runbook.md)
- Deployment guide: [`docs/deployment-guide.md`](docs/deployment-guide.md)
- Rollback playbook: [`docs/rollback-playbook.md`](docs/rollback-playbook.md)
- Operational limits: [`docs/rc-operational-limits.md`](docs/rc-operational-limits.md)
- Latest smoke sign-off record: [`docs/rc-signoff-record.md`](docs/rc-signoff-record.md)
- Final parity closeout summary: [`docs/final-parity-closeout-summary.md`](docs/final-parity-closeout-summary.md)
- ADK delta report: [`docs/adk-delta-report.md`](docs/adk-delta-report.md)

Generate/update the RC smoke sign-off record:

```bash
./scripts/demo/m6_rc_signoff.sh
```

## Browser WS Client

LiveClaw includes a reusable browser WebSocket client for pairing, auth,
session control, and full voice-path validation.
It syncs message templates from `GetDiagnostics` and shows runtime + security + gateway-health + priority badges so protocol changes stay visible in the client per sprint.
The client now supports:
- Saved connection profiles (load/save/delete) for repeatable environments
- Guided one-click workflows:
  - Bootstrap Flow (`Connect -> Pair -> Authenticate -> CreateSession`)
  - Ops Check (`Ping + GetGatewayHealth + GetDiagnostics`)
  - README Summary flow (bootstrap + prompt submission)
- Simple user-facing chat lane built on `SessionPrompt` with message history and Enter-to-send input
- Uploading common audio formats and converting them to PCM16 mono for `SessionAudio`
- Live microphone streaming to `SessionAudio`
- Decoding and playback of `AudioOutput`
- Per-session transcript view for `TranscriptUpdate`
- Direct `SessionToolCall` invocation with tool arguments JSON
- Graph execution trace inspection from `SessionToolResult.graph`
- One-click `Run Read + Summarize` flow that builds a `SessionPrompt` for workspace files
- Prompt tool activity panel that confirms prompt-driven `read_workspace_file` execution details
- Dedicated M4 evidence panel for memory/artifact/resilience snapshots
- One-click memory/artifact `read_workspace_file` probes plus live diagnostics counters in the UI
- Channel bridge panel for `ChannelInbound` routing tests (telegram/slack/webhook/discord/matrix keys)
- Channel outbound poll controls for `GetChannelOutbound` validation
- Channel job scheduler controls for `CreateChannelJob` / `ListChannelJobs` / `CancelChannelJob`

```bash
cd /Users/jameskaranja/Developer/projects/liveclaw
./scripts/ws_client.sh
```

Default URL opened in the browser:
- `http://127.0.0.1:18080/index.html`

Optional host/port override:

```bash
./scripts/ws_client.sh 127.0.0.1 19090
```

Run without auto-opening a browser tab:

```bash
OPEN_BROWSER=0 ./scripts/ws_client.sh
```

## Live Voice E2E Validation

Run the provider-backed end-to-end voice probe (no mocks):

```bash
cd /Users/jameskaranja/Developer/projects/liveclaw
./scripts/demo/m1_voice_e2e_live.sh
```

This script:
- starts LiveClaw (or reuses an already-running gateway),
- creates a WS session,
- generates PCM speech audio via `scripts/generate_e2e_pcm.sh`,
- streams audio chunks through `SessionAudio`,
- asserts `AudioAccepted`, `TranscriptUpdate`, and `AudioOutput` for the created session ID,
- terminates the session and verifies `SessionTerminated`.

Run the prompt-driven tool-call live probe and print model transcript output:

```bash
cd /Users/jameskaranja/Developer/projects/liveclaw
./scripts/demo/m3_prompt_tool_live.sh
```

Useful overrides:
- `LIVECLAW_PROMPT_TEXT="Use utc_time and answer with current UTC only."`
- `LIVECLAW_PROMPT_TEXT="Read README.md with read_workspace_file and summarize in 5 bullets."`
- `LIVECLAW_PROMPT_USE_EXISTING_GATEWAY=auto|always|never`
- `LIVECLAW_PROMPT_TOKEN=<token>` for pairing-required existing gateway runs

Notes:
- Prompt-driven workspace file requests now trigger `read_workspace_file` automatically when a file-read intent/path is detected.
- The gateway emits a `SessionToolResult` event before the model summary so clients can verify the tool invocation.
- `SessionToolCall` works even when graph mode is disabled; the response report marks `execution_mode` as `direct`.

Required:
- `OPENAI_API_KEY` or `LIVECLAW_API_KEY`
- `websocat`

Useful overrides:
- `LIVECLAW_E2E_USE_EXISTING_GATEWAY=auto|always|never` (default `auto`)
- `LIVECLAW_E2E_TOKEN=<token>` for pairing-required existing gateway runs
- `LIVECLAW_E2E_PAIR_CODE=<code>` optional pairing-code override
- `LIVECLAW_E2E_WS_URL=ws://host:port/ws`
- `LIVECLAW_E2E_FORCE_RESPONSE=1` to send `SessionAudioCommit` + `SessionResponseCreate` after upload
- `LIVECLAW_E2E_SAMPLE_RATE=24000` and `LIVECLAW_E2E_TRAILING_SILENCE_MS=1200` for VAD tuning

## Documentation

- [Design Document](docs/design.md) — architecture, data flows, component interfaces
- [Requirements](docs/requirements.md) — user stories, acceptance criteria, correctness properties

## Gateway Protocol

Clients connect via WebSocket and exchange JSON messages:

```jsonc
// Client → Server
{ "type": "Pair", "code": "123456" }
{ "type": "CreateSession", "config": null }
{ "type": "SessionAudio", "session_id": "...", "audio": "<base64>" }
{ "type": "SessionAudioCommit", "session_id": "..." }
{ "type": "SessionResponseCreate", "session_id": "..." }
{ "type": "SessionResponseInterrupt", "session_id": "..." }
{ "type": "SessionPrompt", "session_id": "...", "prompt": "Use add_numbers with a=12 and b=30", "create_response": true }
{ "type": "ChannelInbound", "channel": "slack", "account_id": "T123", "external_user_id": "U77", "text": "hello", "create_response": true }
{ "type": "GetChannelOutbound", "channel": "slack", "account_id": "T123", "external_user_id": "U77", "max_items": 20 }
{ "type": "CreateChannelJob", "channel": "slack", "account_id": "T123", "external_user_id": "U77", "text": "scheduled check", "interval_seconds": 60, "create_response": false }
{ "type": "ListChannelJobs" }
{ "type": "CancelChannelJob", "job_id": "..." }
{ "type": "SessionToolCall", "session_id": "...", "tool_name": "echo_text", "arguments": {"text":"hello"} }
{ "type": "TerminateSession", "session_id": "..." }
{ "type": "GetGatewayHealth" }
{ "type": "PriorityProbe" }
{ "type": "GetDiagnostics" }
{ "type": "Ping" }

// Server → Client
{ "type": "PairSuccess", "token": "..." }
{ "type": "SessionCreated", "session_id": "..." }
{ "type": "AudioAccepted", "session_id": "..." }
{ "type": "AudioCommitted", "session_id": "..." }
{ "type": "ResponseCreateAccepted", "session_id": "..." }
{ "type": "ResponseInterruptAccepted", "session_id": "..." }
{ "type": "PromptAccepted", "session_id": "..." }
{ "type": "ChannelRouted", "channel": "slack", "account_id": "T123", "external_user_id": "U77", "session_id": "..." }
{ "type": "ChannelOutboundBatch", "channel": "slack", "account_id": "T123", "external_user_id": "U77", "items": [ {"id":"...","session_id":"...","text":"...","created_at_unix_ms":0} ] }
{ "type": "ChannelJobCreated", "job": { "job_id":"...", "channel":"slack", "account_id":"T123", "external_user_id":"U77", "text":"scheduled check", "interval_seconds":60, "create_response":false, "created_at_unix_ms":0 } }
{ "type": "ChannelJobs", "jobs": [ { "job_id":"...", "channel":"slack", "account_id":"T123", "external_user_id":"U77", "text":"scheduled check", "interval_seconds":60, "create_response":false, "created_at_unix_ms":0 } ] }
{ "type": "ChannelJobCanceled", "job_id": "..." }
{ "type": "SessionToolResult", "session_id": "...", "tool_name": "echo_text", "result": {"status":"ok","result":{"text":"hello","length":5}}, "graph": {"thread_id":"...","completed":true,"interrupted":false,"events":[...],"final_state":{...}} }
{ "type": "AudioOutput", "session_id": "...", "audio": "<base64>" }
{ "type": "TranscriptUpdate", "session_id": "...", "text": "...", "is_final": true }
{ "type": "PriorityProbeAccepted", "queued_standard": true, "queued_priority": true }
{ "type": "PriorityNotice", "data": { "level": "info", "code": "priority_probe", "message": "Priority channel is active", "session_id": "..." } }
{ "type": "GatewayHealth", "data": { "protocol_version":"2026-02-20", "uptime_seconds": 123, "active_sessions": 1, "active_ws_bindings": 1, "active_priority_bindings": 1, "active_channel_jobs": 1, "require_pairing": true } }
{ "type": "Diagnostics", "data": { "...": "runtime/provider/reconnect/compaction snapshot", "protocol_version": "...", "supported_client_messages": ["..."] } }
{ "type": "Pong" }
```

## Channel Adapter HTTP Endpoints

LiveClaw now exposes token-authenticated ingress adapters that translate
Webhook/Slack/Telegram/Discord/Matrix payloads into the same `ChannelInbound` route used by WS clients.

- `POST /channels/webhook`
- `POST /channels/slack/events`
- `POST /channels/telegram/update`
- `POST /channels/discord/events`
- `POST /channels/matrix/events`
- `POST /channels/outbound/poll`
- `POST /channels/jobs/create`
- `GET /channels/jobs/list`
- `POST /channels/jobs/cancel`
- `POST /channels/webhook/supervised-action`

When pairing is enabled, send the paired token as `Authorization: Bearer <token>`.

Example generic webhook ingress:

```bash
curl -sS -X POST http://127.0.0.1:8420/channels/webhook \
  -H "Authorization: Bearer ${LIVECLAW_TOKEN}" \
  -H "Content-Type: application/json" \
  -d '{
    "channel":"webhook",
    "account_id":"acct-1",
    "external_user_id":"user-1",
    "text":"hello from webhook",
    "create_response": true
  }'
```

Example Slack message ingress:

```bash
curl -sS -X POST http://127.0.0.1:8420/channels/slack/events \
  -H "Authorization: Bearer ${LIVECLAW_TOKEN}" \
  -H "Content-Type: application/json" \
  -d '{
    "team_id":"T123",
    "event": { "type":"message", "user":"U77", "text":"hello from slack" }
  }'
```

Example Telegram message ingress:

```bash
curl -sS -X POST http://127.0.0.1:8420/channels/telegram/update \
  -H "Authorization: Bearer ${LIVECLAW_TOKEN}" \
  -H "Content-Type: application/json" \
  -d '{
    "message": {
      "text":"hello from telegram",
      "chat": { "id": 4455 },
      "from": { "id": 7788 }
    }
  }'
```

Example Discord message ingress:

```bash
curl -sS -X POST http://127.0.0.1:8420/channels/discord/events \
  -H "Authorization: Bearer ${LIVECLAW_TOKEN}" \
  -H "Content-Type: application/json" \
  -d '{
    "type":"MESSAGE_CREATE",
    "guild_id":"G901",
    "author": { "id":"U42", "bot": false },
    "content":"hello from discord"
  }'
```

Example Matrix message ingress:

```bash
curl -sS -X POST http://127.0.0.1:8420/channels/matrix/events \
  -H "Authorization: Bearer ${LIVECLAW_TOKEN}" \
  -H "Content-Type: application/json" \
  -d '{
    "type":"m.room.message",
    "room_id":"!room:example.org",
    "sender":"@alice:example.org",
    "content": { "msgtype":"m.text", "body":"hello from matrix" }
  }'
```

Example outbound poll (adapter delivery pull):

```bash
curl -sS -X POST http://127.0.0.1:8420/channels/outbound/poll \
  -H "Authorization: Bearer ${LIVECLAW_TOKEN}" \
  -H "Content-Type: application/json" \
  -d '{
    "channel":"webhook",
    "account_id":"acct-1",
    "external_user_id":"user-1",
    "max_items": 20
  }'
```

Example channel job create (HTTP scheduler surface):

```bash
curl -sS -X POST http://127.0.0.1:8420/channels/jobs/create \
  -H "Authorization: Bearer ${LIVECLAW_TOKEN}" \
  -H "Content-Type: application/json" \
  -d '{
    "channel":"webhook",
    "account_id":"acct-1",
    "external_user_id":"user-1",
    "text":"scheduled check",
    "interval_seconds": 60,
    "create_response": false
  }'
```

Example channel jobs list:

```bash
curl -sS -X GET http://127.0.0.1:8420/channels/jobs/list \
  -H "Authorization: Bearer ${LIVECLAW_TOKEN}"
```

Example channel job cancel:

```bash
curl -sS -X POST http://127.0.0.1:8420/channels/jobs/cancel \
  -H "Authorization: Bearer ${LIVECLAW_TOKEN}" \
  -H "Content-Type: application/json" \
  -d '{
    "job_id":"<JOB_ID>"
  }'
```

Example webhook-triggered supervised action:

```bash
curl -sS -X POST http://127.0.0.1:8420/channels/webhook/supervised-action \
  -H "Authorization: Bearer ${LIVECLAW_TOKEN}" \
  -H "Content-Type: application/json" \
  -d '{
    "account_id":"acct-1",
    "external_user_id":"user-1",
    "tool_name":"echo_text",
    "arguments":{"text":"hello from webhook"},
    "text":"supervised action trigger"
  }'
```

## Inspiration

LiveClaw is inspired by [ZeroClaw](https://github.com/zeroclaw) and [OpenClaw](https://github.com/openclaw).

## Key Crates

| Crate | Role in LiveClaw |
|---|---|
| [tokio](https://crates.io/crates/tokio) | Async runtime powering all concurrency |
| [axum](https://crates.io/crates/axum) | HTTP/WebSocket server for the gateway |
| [serde](https://crates.io/crates/serde) / [serde_json](https://crates.io/crates/serde_json) | Serialization of protocol messages and config |
| [toml](https://crates.io/crates/toml) | TOML configuration parsing |
| [tokio-tungstenite](https://crates.io/crates/tokio-tungstenite) | WebSocket client support with TLS |
| [tracing](https://crates.io/crates/tracing) | Structured logging and diagnostics |
| [sha2](https://crates.io/crates/sha2) | SHA-256 hashing for pairing token storage |
| [regex](https://crates.io/crates/regex) | PII pattern matching for redaction |
| [chrono](https://crates.io/crates/chrono) | Timestamps for memory entries and audit events |
| [anyhow](https://crates.io/crates/anyhow) / [thiserror](https://crates.io/crates/thiserror) | Error handling |
| [proptest](https://crates.io/crates/proptest) | Property-based testing |

## License

Licensed under the Apache License, Version 2.0. See [LICENSE](LICENSE) for details.
