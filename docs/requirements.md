# Requirements Document

## Introduction

LiveClaw is a voice-first, real-time agent runtime built in Rust that maximizes ADK-Rust crate adoption as its foundation. The architecture uses two crates: `liveclaw-gateway` (WebSocket server, pairing authentication, session routing) and `liveclaw-app` (configuration, wiring, and thin custom layers). LiveClaw eliminates custom voice orchestration by using `adk-realtime::RealtimeAgent` for bidirectional voice streaming with tool dispatch, `adk-runner::Runner` for session lifecycle and plugin management, `adk-session::SessionService` for event-sourced session persistence, `adk-memory::MemoryStore` bridged to `adk-core::Memory` for persistent agent memory, `adk-graph::GraphAgent` for multi-step orchestration with human-in-the-loop interrupts, `adk-auth` for role-based access control and audit logging, `adk-plugin::PluginManager` for PII redaction / rate limiting / memory auto-save / guardrail plugins, and `adk-telemetry` for observability. LiveClaw builds only the unique layers on top: a WebSocket gateway with pairing authentication, shell injection detection (as a `before_tool_callback`), a memory adapter bridging `adk-memory::MemoryStore` to `adk-core::Memory`, and TOML configuration.

## Glossary

- **LiveClaw**: The voice-first real-time agent runtime system being specified, built on ADK-Rust
- **ADK-Rust**: The Agent Development Kit for Rust, providing core agent, tool, session, realtime, runner, memory, graph, auth, plugin, and telemetry crates
- **Gateway**: The transport-layer service (`liveclaw-gateway`) that handles WebSocket client connections, pairing authentication, and session routing to the Runner
- **RealtimeAgent**: The `adk-realtime::RealtimeAgent` — a full Agent trait implementation that handles bidirectional streaming, tool dispatch with callbacks, audio/transcript/speech callbacks, agent transfer, and InvocationContext integration. Built via `RealtimeAgent::builder()`
- **Runner**: The `adk-runner::Runner` that manages session get/create, memory injection, plugin lifecycle, skill injection, agent transfer, event persistence, and context compaction
- **RunnerConfig**: The `adk-runner::RunnerConfig` struct that wires together the agent, session service, artifact service, memory service, plugin manager, and compaction config
- **SessionService**: The `adk-session::SessionService` trait with event-sourced API: `create()`, `get()`, `list()`, `delete()`, `append_event()`. Session exposes `events()`, `state()`, `conversation_history()`
- **Session**: An `adk-session::Session` instance with typed state prefixes (`user:`, `app:`, `temp:`) and event-sourced history
- **MemoryStore**: The `adk-memory::MemoryStore` trait for categorized persistent memory with `store()`, `recall()`, `get()`, `list()`, `forget()`, `count()`, `snapshot()`, `hydrate()` and categories: Core, Session, Conversation, Custom
- **MemoryAdapter**: A custom bridge that implements `adk-core::Memory` (search-only interface) by delegating to `adk-memory::MemoryStore::recall()`
- **GraphAgent**: The `adk-graph::GraphAgent` for modeling voice conversation as a graph with conditional edges, interrupts (Before/After/Dynamic), checkpointing, and cyclic support
- **PluginManager**: The `adk-plugin::PluginManager` that manages plugins with hooks: `before_run`, `after_run`, `on_user_message`, `on_event`
- **Plugin**: An `adk-plugin::Plugin` with a `PluginConfig` specifying name and hook callbacks
- **PairingGuard**: The authentication mechanism that uses a one-time 6-digit pairing code to establish trust between a client and the Gateway
- **AccessControl**: The `adk-auth::AccessControl` component that manages role-based permissions for tool and agent access
- **AuthMiddleware**: The `adk-auth::AuthMiddleware` that wraps tools with access control in bulk
- **AuditSink**: The `adk-auth::AuditSink` trait for logging access control decisions; `FileAuditSink` writes JSONL
- **VadConfig**: The `adk-realtime` voice activity detection configuration with `interrupt_response: true` for server-side VAD and automatic barge-in handling
- **EventsCompactionConfig**: The `adk-runner` configuration for summarizing old events in long-running voice sessions to manage context window size
- **Artifacts**: The `adk-core::Artifacts` trait for saving audio recordings and transcripts; `Runner` creates `ScopedArtifacts` per session
- **before_tool_callback**: A callback on `RealtimeAgent::builder()` that executes before any tool call — used for rate limiting and guardrail checks
- **after_tool_callback**: A callback on `RealtimeAgent::builder()` that executes after any tool call
- **ClientEvent**: A message sent to the realtime provider, defined by `adk-realtime::ClientEvent`
- **ServerEvent**: A message received from the realtime provider, defined by `adk-realtime::ServerEvent`
- **VAD**: Voice Activity Detection — server-side detection of user speech start/stop, configured via `VadConfig`
- **Barge-in**: The ability for a user to interrupt the agent's speech; handled automatically by `RealtimeAgent` via `VadConfig` with `interrupt_response: true`
- **SessionId**: A unique identifier for a voice conversation session
- **InvocationContext**: The `adk-core::InvocationContext` that carries session, memory, artifacts, and agent state through the execution pipeline

## Requirements

### Requirement 1: Gateway WebSocket Protocol

**User Story:** As a client application, I want to connect to the LiveClaw gateway over WebSocket, so that I can establish and manage voice sessions remotely.

#### Acceptance Criteria

1. THE Gateway SHALL accept WebSocket connections on a configurable host and port
2. THE Gateway SHALL bind to localhost (127.0.0.1) by default
3. WHEN a client connects via WebSocket, THE Gateway SHALL require authentication via a valid pairing token before accepting session commands
4. WHEN an authenticated client sends a `CreateSession` message, THE Gateway SHALL invoke the Runner to create a new voice session and return the assigned SessionId
5. WHEN an authenticated client sends a `TerminateSession` message, THE Gateway SHALL terminate the specified voice session via the Runner and release its resources
6. THE Gateway SHALL serialize all protocol messages (`GatewayMessage` and `GatewayResponse`) to JSON format
7. THE Gateway SHALL deserialize all protocol messages from JSON format
8. FOR ALL valid `GatewayMessage` values, serializing then deserializing SHALL produce an equivalent message (round-trip property)
9. FOR ALL valid `GatewayResponse` values, serializing then deserializing SHALL produce an equivalent response (round-trip property)
10. IF an unauthenticated client sends a session command, THEN THE Gateway SHALL reject the command with an authentication error response
11. WHEN a client sends a `Ping` message, THE Gateway SHALL respond with a `Pong` message
12. WHEN a client sends a `SessionAudio` message, THE Gateway SHALL forward the audio data to the Runner's active RealtimeAgent session

### Requirement 2: Pairing Authentication

**User Story:** As a user, I want to securely pair my client with the LiveClaw gateway, so that only authorized clients can control voice sessions.

#### Acceptance Criteria

1. WHEN the Gateway starts with no existing pairing tokens, THE PairingGuard SHALL generate a 6-digit pairing code and display it to the operator
2. WHEN a client submits the correct pairing code, THE PairingGuard SHALL generate a cryptographic token, store its SHA-256 hash, and return the token to the client
3. WHEN a client submits an incorrect pairing code, THE PairingGuard SHALL reject the attempt and increment the failed attempt counter
4. WHEN the failed attempt counter reaches a configurable maximum (default 5), THE PairingGuard SHALL lock out pairing attempts for a configurable duration
5. WHEN a client presents a valid pairing token, THE PairingGuard SHALL authenticate the client using constant-time comparison of the token hash
6. WHEN the Gateway is configured with `require_pairing: false`, THE PairingGuard SHALL allow all connections without authentication

### Requirement 3: Role-Based Access Control via adk-auth

**User Story:** As a system administrator, I want to enforce role-based access control on tool execution, so that untrusted audio inputs cannot trigger dangerous operations.

#### Acceptance Criteria

1. THE LiveClaw runtime SHALL define three roles using `adk-auth::Role`: `ReadOnly` (no tool permissions), `Supervised` (explicitly allowed tools only), and `Full` (all tools permitted via `Permission::AllTools`)
2. THE LiveClaw runtime SHALL configure an `adk-auth::AccessControl` instance with the defined roles and assign sessions to roles based on configuration
3. WHEN a tool execution is requested, THE AccessControl SHALL verify the session's role permits the tool via `check(user, Permission::Tool(name))`
4. WHEN a tool execution is denied by AccessControl, THE system SHALL return a denial result with a descriptive reason to the realtime provider
5. THE LiveClaw runtime SHALL wrap all registered tools using `adk-auth::AuthMiddleware::protect_all(tools)` before registering them with `RealtimeAgent::builder()`
6. THE Supervised role SHALL use `interrupt_before` on tool nodes in the GraphAgent for human-in-the-loop approval of tool calls
7. WHEN the rate limit is exceeded, THE before_tool_callback SHALL return a denial result indicating rate limit exceeded

### Requirement 4: Content Filtering and Guardrail Plugins

**User Story:** As a system administrator, I want to filter harmful content and redact PII from voice transcripts, so that the system is safe and privacy-compliant.

#### Acceptance Criteria

1. THE LiveClaw runtime SHALL implement a shell injection detection `before_tool_callback` on the RealtimeAgent that detects shell injection patterns including semicolons, backticks, `$(...)`, pipe chains, `&&`, `||`, redirect operators, and newlines in tool arguments
2. WHEN a tool argument contains a shell injection pattern, THE before_tool_callback SHALL block the tool execution and return a failure result with the detected pattern
3. THE LiveClaw runtime SHALL register a PII redaction plugin via `PluginManager` that intercepts transcript events via the `on_event` hook and redacts email addresses, phone numbers, SSNs, credit card numbers, and IP addresses before they are persisted
4. WHEN a transcript event is emitted, THE PII redaction plugin SHALL redact PII from the transcript text and pass the redacted event to the session's `append_event()`
5. THE LiveClaw runtime SHALL register a guardrail plugin via `PluginManager` that validates user messages via the `on_user_message` hook for harmful content

### Requirement 5: Audit Logging via adk-auth

**User Story:** As a system operator, I want all tool access decisions logged, so that I can audit security events and investigate incidents.

#### Acceptance Criteria

1. THE LiveClaw runtime SHALL configure an `adk-auth::FileAuditSink` that writes audit events in JSONL format to a configurable file path
2. THE AccessControl SHALL use `check_and_audit(user, permission)` for all tool access checks to automatically log decisions
3. WHEN a tool access is checked, THE AuditSink SHALL record an `AuditEvent` containing the tool name, session identifier, outcome (Allowed/Denied), and timestamp

### Requirement 6: Voice Orchestration via RealtimeAgent

**User Story:** As a user, I want to have real-time voice conversations with an AI agent, so that I can interact naturally using speech.

#### Acceptance Criteria

1. THE LiveClaw runtime SHALL build a `RealtimeAgent` via `RealtimeAgent::builder("voice_assistant")` with model, instruction, voice, server VAD, tools, sub-agents, and callbacks
2. THE RealtimeAgent SHALL be configured with `server_vad()` for server-side voice activity detection with `interrupt_response: true` for automatic barge-in handling
3. THE RealtimeAgent SHALL register an `on_audio` callback that forwards audio output to the Gateway for client playback
4. THE RealtimeAgent SHALL register an `on_transcript` callback that triggers PII redaction and stores the redacted transcript via `SessionService::append_event()`
5. THE RealtimeAgent SHALL register `on_speech_started` and `on_speech_stopped` callbacks for latency tracking
6. THE RealtimeAgent SHALL register tools wrapped by `AuthMiddleware::protect_all()` for role-based access control
7. THE RealtimeAgent SHALL register a `before_tool_callback` for rate limiting and shell injection detection
8. THE RealtimeAgent SHALL register an `after_tool_callback` for recording tool execution metrics
9. WHEN a voice session is terminated, THE Runner SHALL close the session and release all resources
10. IF the connection to the realtime provider drops unexpectedly, THEN THE RealtimeAgent SHALL attempt reconnection with exponential backoff up to a configurable maximum retry count

### Requirement 7: Barge-in via Server-Side VAD

**User Story:** As a user, I want to interrupt the agent while it is speaking, so that I can redirect the conversation naturally.

#### Acceptance Criteria

1. THE RealtimeAgent SHALL be configured with `VadConfig` with `interrupt_response: true` for automatic server-side barge-in handling
2. WHEN the server-side VAD detects user speech during agent audio output, THE RealtimeAgent SHALL automatically interrupt the current response and begin processing the new user input
3. WHEN a barge-in occurs, THE RealtimeAgent SHALL discard any buffered agent audio not yet sent to the client
4. WHEN a barge-in occurs, THE voice session SHALL remain active and continue accepting user audio without requiring a new session

### Requirement 8: Tool Execution via before_tool_callback

**User Story:** As a developer, I want the voice agent to dispatch tool calls through ADK-Rust's tool system with access control, so that the agent can perform actions during a conversation safely.

#### Acceptance Criteria

1. THE RealtimeAgent SHALL register tools using `adk-tool::FunctionTool` via the `.tool()` builder method
2. WHEN the realtime provider requests a tool call, THE RealtimeAgent SHALL delegate execution through the ADK-Rust tool pipeline with registered callbacks
3. WHEN a tool execution is requested, THE before_tool_callback SHALL check the per-session rate limit and block if exceeded
4. WHEN a tool execution is requested, THE before_tool_callback SHALL validate tool arguments against shell injection patterns and block if detected
5. WHEN a tool execution completes, THE after_tool_callback SHALL record the tool name and execution duration as a metric via adk-telemetry

### Requirement 9: Runner Integration

**User Story:** As a developer, I want the Runner to manage the full session lifecycle, so that session creation, memory injection, plugin hooks, and event persistence are handled consistently.

#### Acceptance Criteria

1. THE LiveClaw runtime SHALL create a `Runner` via `Runner::new(RunnerConfig { ... })` with the RealtimeAgent (or GraphAgent wrapping it), SessionService, artifact service, memory service, plugin manager, and compaction config
2. WHEN a new voice session is requested, THE Runner SHALL create or retrieve a session via `SessionService::create()` or `SessionService::get()`
3. WHEN the Runner starts a session, THE Runner SHALL inject the memory service into the `InvocationContext` so the agent can access persistent memory
4. WHEN the Runner starts a session, THE Runner SHALL execute all registered plugin `before_run` hooks
5. WHEN the Runner completes a session, THE Runner SHALL execute all registered plugin `after_run` hooks
6. THE Runner SHALL persist all events to the session via `SessionService::append_event()` during the conversation
7. THE Runner SHALL return an event stream from `runner.run(user_id, session_id, user_content)` that the Gateway can consume

### Requirement 10: Event-Sourced Session Management

**User Story:** As a developer, I want sessions to use the event-sourced SessionService API, so that conversation history and state are properly tracked.

#### Acceptance Criteria

1. THE LiveClaw runtime SHALL use `adk-session::SessionService` with its event-sourced API: `create()`, `get()`, `list()`, `delete()`, `append_event()`
2. WHEN a transcript is produced, THE system SHALL store it via `SessionService::append_event()` as a session event
3. THE Session SHALL expose `events()` for full event history, `state()` for typed state with prefixes (`user:`, `app:`, `temp:`), and `conversation_history()` for the conversation thread
4. THE LiveClaw runtime SHALL use `adk-session::InMemorySessionService` as the default implementation

### Requirement 11: Persistent Agent Memory

**User Story:** As a user, I want the voice agent to remember context across sessions, so that I do not have to repeat information in every conversation.

#### Acceptance Criteria

1. THE LiveClaw runtime SHALL implement a `MemoryAdapter` that bridges `adk-memory::MemoryStore` to `adk-core::Memory` by delegating `search()` to `MemoryStore::recall()`
2. THE MemoryAdapter SHALL pass the search query and a configurable result limit to `MemoryStore::recall()` and convert the results to `adk-core::MemoryEntry` format
3. THE LiveClaw runtime SHALL register a memory auto-save plugin via `PluginManager` that stores conversation summaries to `MemoryStore` via the `after_run` hook using categorized storage (Core, Session, Conversation)
4. THE Runner SHALL inject the MemoryAdapter into the `InvocationContext` via the `memory_service` field so the agent can search past memories during conversation
5. FOR ALL valid MemoryEntry values, storing via `MemoryStore::store()` then recalling via `MemoryStore::recall()` with a matching query SHALL return results containing the stored entry

### Requirement 12: Agent Orchestration via GraphAgent

**User Story:** As a developer, I want to model the voice conversation as a graph with conditional routing and human-in-the-loop approval, so that complex multi-step workflows are supported.

#### Acceptance Criteria

1. THE LiveClaw runtime SHALL optionally wrap the RealtimeAgent in a `GraphAgent` built via `GraphAgent::builder("voice_orchestrator")` with nodes, edges, conditional edges, and a checkpointer
2. THE GraphAgent SHALL include an agent node wrapping the RealtimeAgent and a tools node for tool execution
3. THE GraphAgent SHALL use conditional edges to route between the agent node and tools node based on whether tool calls are pending
4. WHERE the session role is `Supervised`, THE GraphAgent SHALL configure `interrupt_before(&["tools"])` for human-in-the-loop approval of tool calls
5. THE GraphAgent SHALL use `MemoryCheckpointer::new()` for graph state checkpointing
6. THE GraphAgent SHALL set a configurable `recursion_limit` (default 25) to prevent infinite loops in cyclic graphs

### Requirement 13: Plugin Architecture

**User Story:** As a developer, I want to extend the runtime with plugins for cross-cutting concerns, so that PII redaction, rate limiting, memory auto-save, and guardrail checks are modular and composable.

#### Acceptance Criteria

1. THE LiveClaw runtime SHALL create a `PluginManager` with plugins for: PII redaction, rate limiting, memory auto-save, and guardrail content filtering
2. THE PII redaction plugin SHALL use the `on_event` hook to intercept transcript events and redact PII before persistence
3. THE rate limiting plugin SHALL use the `on_event` hook to track tool execution counts per session and block when the configurable limit is exceeded
4. THE memory auto-save plugin SHALL use the `after_run` hook to store a conversation summary in `MemoryStore` with the `Conversation` category
5. THE guardrail plugin SHALL use the `on_user_message` hook to validate incoming user messages for harmful content patterns
6. THE Runner SHALL receive the `PluginManager` via `RunnerConfig::plugin_manager` and execute plugin hooks at the appropriate lifecycle points

### Requirement 14: Context Compaction

**User Story:** As a system operator, I want long-running voice sessions to automatically compact their event history, so that the context window does not overflow.

#### Acceptance Criteria

1. THE Runner SHALL be configured with `EventsCompactionConfig` to enable automatic summarization of old events in long-running sessions
2. WHEN the event count in a session exceeds the compaction threshold, THE Runner SHALL summarize older events and replace them with a compact summary event
3. THE compaction configuration SHALL specify the maximum event count before compaction and the summarization strategy

### Requirement 15: Artifact Support

**User Story:** As a system operator, I want to save audio recordings and transcripts as artifacts, so that I can review and archive voice interactions.

#### Acceptance Criteria

1. THE Runner SHALL be configured with an artifact service implementing `adk-core::Artifacts` for saving audio recordings and transcripts
2. THE Runner SHALL create `ScopedArtifacts` per session for isolated artifact storage
3. WHEN a voice session produces a final transcript, THE system SHALL save the transcript as an artifact via the artifact service
4. WHEN a voice session produces audio output, THE system SHALL optionally save the audio recording as an artifact based on configuration

### Requirement 16: Configuration Management

**User Story:** As a system administrator, I want to configure LiveClaw through a TOML configuration file, so that I can customize the runtime behavior without code changes.

#### Acceptance Criteria

1. THE LiveClaw runtime SHALL load configuration from a TOML file at a configurable path
2. THE LiveClaw configuration SHALL include sections for: gateway settings (host, port, require_pairing), voice settings (provider, model, voice, audio format), security settings (role assignment, tool allowlist, rate limit, audit log path), plugin settings (enable PII redaction, enable memory auto-save, custom blocked keywords), memory settings (memory store backend, recall limit), graph settings (enable graph orchestration, recursion limit), compaction settings (enable compaction, max events threshold), artifact settings (enable artifact storage, storage path), pairing settings (max attempts, lockout duration), and telemetry settings (OTLP enabled)
3. WHEN a configuration value is missing, THE LiveClaw runtime SHALL use documented default values
4. IF the configuration file contains invalid TOML syntax, THEN THE LiveClaw runtime SHALL return a descriptive parse error
5. IF the configuration file contains unknown fields, THEN THE LiveClaw runtime SHALL ignore unknown fields and log a warning
6. FOR ALL valid configuration values, serializing to TOML then deserializing SHALL produce an equivalent configuration (round-trip property)

### Requirement 17: Observability via adk-telemetry

**User Story:** As a system operator, I want to monitor LiveClaw's performance and health using structured telemetry, so that I can detect issues and measure voice interaction quality.

#### Acceptance Criteria

1. THE LiveClaw runtime SHALL initialize telemetry using `adk-telemetry::init_telemetry("liveclaw")` for structured logging and tracing
2. WHERE OTLP export is configured, THE LiveClaw runtime SHALL use `adk-telemetry::init_with_otlp("liveclaw")` for Jaeger/Datadog integration
3. THE LiveClaw runtime SHALL use `adk-telemetry` re-exported `tracing` macros (`info`, `warn`, `error`, `debug`, `instrument`) for all logging
4. THE LiveClaw runtime SHALL use `adk-telemetry` re-exported `opentelemetry::metrics` for recording active session count, tool call count, error count, and latency histograms
5. THE LiveClaw runtime SHALL record latency metrics for speech-to-transcript duration, turn-to-first-audio duration, and barge-in response time
6. THE LiveClaw runtime SHALL expose a health check endpoint that reports the Gateway's readiness status

### Requirement 18: Workspace and Crate Structure

**User Story:** As a developer, I want a well-organized Rust workspace with separate crates that depend on ADK-Rust, so that I can build, test, and deploy components independently.

#### Acceptance Criteria

1. THE LiveClaw project SHALL be organized as a Cargo workspace with two crates: `liveclaw-gateway` (WebSocket protocol, pairing auth, session routing) and `liveclaw-app` (entry point, config, RealtimeAgent/Runner/GraphAgent wiring, MemoryAdapter, plugins, security callbacks)
2. THE `liveclaw-gateway` crate SHALL depend on `adk-auth` for access control types used in session context
3. THE `liveclaw-app` crate SHALL depend on `adk-core`, `adk-realtime`, `adk-session`, `adk-tool`, `adk-runner`, `adk-auth`, `adk-memory`, `adk-graph`, `adk-plugin`, `adk-telemetry`, and `liveclaw-gateway`
4. THE LiveClaw workspace SHALL include a CI configuration that runs `cargo check`, `cargo test`, `cargo clippy`, and `cargo fmt --check`
