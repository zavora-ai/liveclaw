# Changelog

All notable changes to LiveClaw will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.0] - 2025-02-18

### Added

- Cargo workspace with two crates: `liveclaw-gateway` and `liveclaw-app`
- WebSocket gateway server built on axum with JSON protocol
- Gateway protocol messages: Pair, CreateSession, TerminateSession, SessionAudio, Ping/Pong
- PairingGuard authentication with 6-digit one-time codes and brute-force lockout
- Role-based access control (ReadOnly, Supervised, Full) via `adk-auth`
- JSONL audit logging via `FileAuditSink`
- RealtimeAgent integration with server-side VAD and barge-in support
- Shell injection detection in tool arguments via `before_tool_callback`
- Per-session rate limiting on tool executions
- PII redaction plugin for email, phone, SSN, credit card, and IP patterns
- Memory auto-save plugin storing conversation summaries via `adk-memory`
- Guardrail content filtering plugin with configurable blocked keywords
- MemoryAdapter bridging `adk-memory::MemoryStore` to `adk-core::Memory`
- Optional GraphAgent wrapping with conditional edges and human-in-the-loop interrupts
- Runner integration for session lifecycle, plugin hooks, and event persistence
- TOML-based configuration with sensible defaults for all sections
- OpenTelemetry support via `adk-telemetry` with optional OTLP export
- Health check endpoint at `/health`
- Dev setup script and development config template
- Property-based tests with `proptest` for protocol round-trips, pairing, security, and config
- Apache 2.0 license
- Project README with architecture overview, quick start, and protocol reference
- Design and requirements documentation in `docs/`
