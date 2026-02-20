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
# Clone ADK-Rust as a sibling
cd ..
git clone <adk-rust-repo-url> adk-rust
cd liveclaw

# Run the dev setup script
chmod +x dev/setup.sh
./dev/setup.sh

# Edit the config with your API key
vim liveclaw.toml

# Run
cargo run -- liveclaw.toml
```

## Configuration

LiveClaw loads configuration from a TOML file. See [`dev/liveclaw.dev.toml`](dev/liveclaw.dev.toml) for a complete example with all options.

Key sections:

| Section | Description |
|---|---|
| `[gateway]` | Host, port, pairing toggle |
| `[voice]` | Provider, API key, model, voice, instructions |
| `[security]` | Default role, tool allowlist, rate limit, audit log path |
| `[plugin]` | PII redaction, memory auto-save, blocked keywords |
| `[memory]` | Backend type, recall limit |
| `[graph]` | Enable graph orchestration, recursion limit |
| `[compaction]` | Transcript compaction toggle and memory threshold |
| `[artifact]` | Artifact persistence toggle and storage path |
| `[pairing]` | Max attempts, lockout duration |
| `[resilience]` | Provider reconnect policy (attempts and backoff) |
| `[telemetry]` | OTLP export toggle |

Missing fields use documented defaults. Unknown fields are ignored.

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

## Browser WS Client

LiveClaw includes a reusable browser WebSocket client for pairing, auth,
session control, and SessionAudio chunk streaming.

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
{ "type": "TerminateSession", "session_id": "..." }
{ "type": "Ping" }

// Server → Client
{ "type": "PairSuccess", "token": "..." }
{ "type": "SessionCreated", "session_id": "..." }
{ "type": "AudioOutput", "session_id": "...", "audio": "<base64>" }
{ "type": "TranscriptUpdate", "session_id": "...", "text": "...", "is_final": true }
{ "type": "Pong" }
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
