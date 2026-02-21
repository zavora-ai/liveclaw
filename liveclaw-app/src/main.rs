//! LiveClaw entry point — wires ADK-Rust realtime sessions to the gateway.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use tokio::sync::{mpsc, RwLock};
use tokio::task::JoinHandle;
use tracing::{info, warn};

use adk_artifact::{ArtifactService, SaveRequest};
use adk_auth::{AccessControl, AuthMiddleware, FileAuditSink, Permission, Role};
use adk_core::{
    CallbackContext, Content, EventActions, MemoryEntry as CoreMemoryEntry, Part, ReadonlyContext,
    Tool, ToolContext,
};
use adk_graph::{
    ExecutionConfig as GraphExecutionConfig, GraphAgent, GraphError, MemoryCheckpointer,
    NodeContext, NodeOutput, State, END, START,
};
use adk_memory::MemoryService;
use adk_realtime::config::ToolDefinition;
use adk_realtime::error::RealtimeError;
use adk_realtime::events::ToolCall;
use adk_realtime::openai::OpenAIRealtimeModel;
use adk_realtime::runner::{EventHandler, ToolHandler};
use adk_realtime::{RealtimeConfig, RealtimeRunner};
use adk_telemetry::{init_telemetry, init_with_otlp};
use serde_json::{json, Value};

use liveclaw_app::config::{
    CompactionConfig, LiveClawConfig, ProviderProfileConfig, ProvidersConfig, ResilienceConfig,
    RuntimeKind, SecurityConfig as AppSecurityConfig, VoiceConfig,
};
use liveclaw_app::storage::{build_memory_service, FileArtifactService};
use liveclaw_app::tools::{build_baseline_tools_with_workspace, WorkspaceToolPolicy};
use liveclaw_gateway::pairing::PairingGuard;
use liveclaw_gateway::protocol::{
    supported_client_message_types, supported_server_response_types, GraphExecutionEvent,
    GraphExecutionReport, RuntimeDiagnostics, SessionConfig, PROTOCOL_VERSION,
};
use liveclaw_gateway::server::{
    Gateway, GatewayConfig, RunnerHandle, SessionAudioOutput, SessionTranscriptOutput,
};

/// A connected realtime session and its background event loop task.
struct RealtimeSession {
    runtime: Arc<dyn SessionRuntime>,
    run_task: JoinHandle<()>,
    user_id: String,
    role: String,
    tools: HashMap<String, Arc<dyn Tool>>,
    graph_enabled: bool,
    graph_recursion_limit: usize,
    shutdown: Arc<AtomicBool>,
}

#[async_trait]
trait SessionRuntime: Send + Sync {
    async fn connect(&self) -> Result<()>;
    async fn run(&self) -> Result<()>;
    async fn close(&self) -> Result<()>;
    async fn send_audio_base64(&self, audio_base64: &str) -> Result<()>;
    async fn send_text(&self, text: &str) -> Result<()>;
    async fn commit_audio(&self) -> Result<()>;
    async fn create_response(&self) -> Result<()>;
    async fn interrupt_response(&self) -> Result<()>;
}

struct RealtimeRunnerRuntime {
    runner: Arc<RealtimeRunner>,
}

impl RealtimeRunnerRuntime {
    fn new(runner: Arc<RealtimeRunner>) -> Self {
        Self { runner }
    }
}

#[async_trait]
impl SessionRuntime for RealtimeRunnerRuntime {
    async fn connect(&self) -> Result<()> {
        self.runner
            .connect()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to connect realtime session: {}", e))
    }

    async fn run(&self) -> Result<()> {
        self.runner
            .run()
            .await
            .map_err(|e| anyhow::anyhow!("Realtime runner loop ended: {}", e))
    }

    async fn close(&self) -> Result<()> {
        self.runner
            .close()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to close realtime session cleanly: {}", e))
    }

    async fn send_audio_base64(&self, audio_base64: &str) -> Result<()> {
        self.runner
            .send_audio(audio_base64)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to send audio to realtime provider: {}", e))
    }

    async fn send_text(&self, text: &str) -> Result<()> {
        self.runner
            .send_text(text)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to send text to realtime provider: {}", e))
    }

    async fn commit_audio(&self) -> Result<()> {
        self.runner
            .commit_audio()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to commit realtime audio buffer: {}", e))
    }

    async fn create_response(&self) -> Result<()> {
        self.runner
            .create_response()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to request realtime response: {}", e))
    }

    async fn interrupt_response(&self) -> Result<()> {
        self.runner
            .interrupt()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to interrupt realtime response: {}", e))
    }
}

const GRAPH_TOOL_REQUEST_KEY: &str = "tool_request";
const GRAPH_PENDING_TOOL_CALLS_KEY: &str = "pending_tool_calls";
const GRAPH_TOOL_RESULTS_KEY: &str = "tool_results";
const GRAPH_TOOLS_EXECUTED_COUNT_KEY: &str = "tools_executed_count";
const GRAPH_TRACE_KEY: &str = "graph_trace";

fn graph_event_json(event: &GraphExecutionEvent) -> Value {
    json!({
        "step": event.step,
        "node": event.node,
        "action": event.action,
        "detail": event.detail,
    })
}

fn append_graph_trace(state: &State, event: GraphExecutionEvent) -> Value {
    let mut trace = state
        .get(GRAPH_TRACE_KEY)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    trace.push(graph_event_json(&event));
    Value::Array(trace)
}

fn graph_trace_from_state(state: &State) -> Vec<GraphExecutionEvent> {
    state
        .get(GRAPH_TRACE_KEY)
        .and_then(Value::as_array)
        .map(|events| {
            events
                .iter()
                .filter_map(|item| serde_json::from_value::<GraphExecutionEvent>(item.clone()).ok())
                .collect()
        })
        .unwrap_or_default()
}

fn graph_state_to_json(state: State) -> Value {
    Value::Object(state.into_iter().collect())
}

#[derive(Clone)]
struct ReconnectPolicy {
    enable_reconnect: bool,
    max_attempts: u32,
    initial_backoff: Duration,
    max_backoff: Duration,
}

impl From<&ResilienceConfig> for ReconnectPolicy {
    fn from(value: &ResilienceConfig) -> Self {
        let initial_backoff_ms = value.initial_backoff_ms.max(1);
        let max_backoff_ms = value.max_backoff_ms.max(initial_backoff_ms);
        Self {
            enable_reconnect: value.enable_reconnect,
            max_attempts: value.max_reconnect_attempts,
            initial_backoff: Duration::from_millis(initial_backoff_ms),
            max_backoff: Duration::from_millis(max_backoff_ms),
        }
    }
}

impl ReconnectPolicy {
    fn backoff_for_attempt(&self, attempt: u32) -> Duration {
        if attempt == 0 {
            return Duration::from_millis(0);
        }

        let exponent = attempt.saturating_sub(1).min(16);
        let multiplier = 1u128 << exponent;
        let backoff_ms = self.initial_backoff.as_millis().saturating_mul(multiplier);
        let bounded_ms = backoff_ms.min(self.max_backoff.as_millis());
        Duration::from_millis(bounded_ms as u64)
    }
}

#[derive(Clone)]
struct MemoryCompactionPolicy {
    enabled: bool,
    max_events_threshold: usize,
}

impl From<&CompactionConfig> for MemoryCompactionPolicy {
    fn from(value: &CompactionConfig) -> Self {
        Self {
            enabled: value.enable_compaction,
            max_events_threshold: value.max_events_threshold,
        }
    }
}

#[derive(Debug, Clone)]
enum ProviderKind {
    OpenAI,
    OpenAICompatible,
}

#[derive(Debug, Clone)]
struct ProviderSelection {
    kind: ProviderKind,
    profile_name: String,
    provider_label: String,
    api_key: String,
    default_model: String,
    base_url: Option<String>,
}

#[derive(Clone)]
struct ToolSecurityConfig {
    default_role: String,
    tool_allowlist: Vec<String>,
    audit_log_path: String,
    workspace_root: String,
    forbidden_tool_paths: Vec<String>,
    principal_allowlist: Vec<String>,
    deny_by_default_principal_allowlist: bool,
    allow_public_bind: bool,
}

impl From<&AppSecurityConfig> for ToolSecurityConfig {
    fn from(value: &AppSecurityConfig) -> Self {
        Self {
            default_role: value.default_role.clone(),
            tool_allowlist: value.tool_allowlist.clone(),
            audit_log_path: value.audit_log_path.clone(),
            workspace_root: value.workspace_root.clone(),
            forbidden_tool_paths: value.forbidden_tool_paths.clone(),
            principal_allowlist: value.principal_allowlist.clone(),
            deny_by_default_principal_allowlist: value.deny_by_default_principal_allowlist,
            allow_public_bind: value.allow_public_bind,
        }
    }
}

#[derive(Default)]
struct ToolExecutionMetrics {
    total_calls: AtomicU64,
    failed_calls: AtomicU64,
    total_duration_millis: AtomicU64,
}

#[derive(Default)]
struct RuntimeDiagnosticsCounters {
    reconnect_attempts_total: AtomicU64,
    reconnect_successes_total: AtomicU64,
    reconnect_failures_total: AtomicU64,
    compactions_applied_total: AtomicU64,
}

impl ToolExecutionMetrics {
    fn record(&self, tool_name: &str, elapsed: Duration, success: bool) {
        let total_calls = self.total_calls.fetch_add(1, Ordering::SeqCst) + 1;
        let failed_calls = if success {
            self.failed_calls.load(Ordering::SeqCst)
        } else {
            self.failed_calls.fetch_add(1, Ordering::SeqCst) + 1
        };
        self.total_duration_millis
            .fetch_add(elapsed.as_millis() as u64, Ordering::SeqCst);

        info!(
            tool = tool_name,
            duration_ms = elapsed.as_millis(),
            success = success,
            total_calls = total_calls,
            failed_calls = failed_calls,
            "Tool execution completed"
        );
    }

    #[cfg(test)]
    fn snapshot(&self) -> (u64, u64, u64) {
        (
            self.total_calls.load(Ordering::SeqCst),
            self.failed_calls.load(Ordering::SeqCst),
            self.total_duration_millis.load(Ordering::SeqCst),
        )
    }
}

struct ToolInvocationContext {
    function_call_id: String,
    user_id: String,
    session_id: String,
    content: Content,
    actions: Mutex<EventActions>,
}

impl ToolInvocationContext {
    fn new(function_call_id: String, user_id: String, session_id: String) -> Self {
        Self {
            function_call_id,
            user_id,
            session_id,
            content: Content::new("user"),
            actions: Mutex::new(EventActions::default()),
        }
    }
}

#[async_trait]
impl ReadonlyContext for ToolInvocationContext {
    fn invocation_id(&self) -> &str {
        &self.function_call_id
    }

    fn agent_name(&self) -> &str {
        "liveclaw-tool-runtime"
    }

    fn user_id(&self) -> &str {
        &self.user_id
    }

    fn app_name(&self) -> &str {
        "liveclaw"
    }

    fn session_id(&self) -> &str {
        &self.session_id
    }

    fn branch(&self) -> &str {
        ""
    }

    fn user_content(&self) -> &Content {
        &self.content
    }
}

#[async_trait]
impl CallbackContext for ToolInvocationContext {
    fn artifacts(&self) -> Option<Arc<dyn adk_core::Artifacts>> {
        None
    }
}

#[async_trait]
impl ToolContext for ToolInvocationContext {
    fn function_call_id(&self) -> &str {
        &self.function_call_id
    }

    fn actions(&self) -> EventActions {
        self.actions
            .lock()
            .expect("tool actions mutex poisoned")
            .clone()
    }

    fn set_actions(&self, actions: EventActions) {
        *self.actions.lock().expect("tool actions mutex poisoned") = actions;
    }

    async fn search_memory(&self, _query: &str) -> adk_core::Result<Vec<CoreMemoryEntry>> {
        Ok(Vec::new())
    }
}

struct AdkToolHandler {
    tool: Arc<dyn Tool>,
    session_id: String,
    user_id: String,
    metrics: Arc<ToolExecutionMetrics>,
}

#[async_trait]
impl ToolHandler for AdkToolHandler {
    async fn execute(&self, call: &ToolCall) -> adk_realtime::Result<serde_json::Value> {
        let start = Instant::now();
        let ctx = Arc::new(ToolInvocationContext::new(
            call.call_id.clone(),
            self.user_id.clone(),
            self.session_id.clone(),
        )) as Arc<dyn ToolContext>;

        match self.tool.execute(ctx, call.arguments.clone()).await {
            Ok(output) => {
                self.metrics.record(self.tool.name(), start.elapsed(), true);
                Ok(output)
            }
            Err(e) => {
                self.metrics
                    .record(self.tool.name(), start.elapsed(), false);
                Err(RealtimeError::ToolError(e.to_string()))
            }
        }
    }
}

async fn execute_session_tool_call_graph(
    session_id: &str,
    user_id: &str,
    role: &str,
    tool_name: &str,
    arguments: Value,
    tools: HashMap<String, Arc<dyn Tool>>,
    recursion_limit: usize,
) -> Result<(Value, GraphExecutionReport)> {
    let thread_id = format!(
        "{}-graph-{}",
        session_id,
        chrono::Utc::now().timestamp_millis()
    );

    let tools = Arc::new(tools);
    let available_tools = {
        let mut names = tools.keys().cloned().collect::<Vec<_>>();
        names.sort();
        Arc::new(names)
    };
    let user_id = user_id.to_string();
    let session_id_owned = session_id.to_string();

    let mut builder = GraphAgent::builder("liveclaw_tool_graph")
        .node_fn("resolve_tool_call", |ctx: NodeContext| async move {
            let tool_request = ctx
                .state
                .get(GRAPH_TOOL_REQUEST_KEY)
                .cloned()
                .unwrap_or_else(|| json!({}));
            let pending = if tool_request.is_null() {
                Vec::new()
            } else {
                vec![tool_request]
            };
            let detail = if pending.is_empty() {
                "no tool call queued".to_string()
            } else {
                format!("queued {} tool call(s)", pending.len())
            };

            Ok(NodeOutput::new()
                .with_update(GRAPH_PENDING_TOOL_CALLS_KEY, Value::Array(pending))
                .with_update(
                    GRAPH_TRACE_KEY,
                    append_graph_trace(
                        &ctx.state,
                        GraphExecutionEvent {
                            step: ctx.step,
                            node: "resolve_tool_call".to_string(),
                            action: "node_end".to_string(),
                            detail,
                        },
                    ),
                ))
        })
        .node_fn("tools", move |ctx: NodeContext| {
            let tools = tools.clone();
            let available_tools = available_tools.clone();
            let user_id = user_id.clone();
            let session_id = session_id_owned.clone();
            async move {
                let pending_calls = ctx
                    .state
                    .get(GRAPH_PENDING_TOOL_CALLS_KEY)
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();

                let mut results = Vec::with_capacity(pending_calls.len());
                for (idx, call) in pending_calls.iter().enumerate() {
                    let called_tool_name = call
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .trim()
                        .to_string();
                    let call_arguments =
                        call.get("arguments").cloned().unwrap_or_else(|| json!({}));

                    if called_tool_name.is_empty() {
                        results.push(json!({
                            "tool": "",
                            "status": "error",
                            "message": "Tool call missing 'name'",
                        }));
                        continue;
                    }

                    let Some(tool) = tools.get(&called_tool_name) else {
                        results.push(json!({
                            "tool": called_tool_name,
                            "status": "error",
                            "message": "Tool is not registered for this session",
                            "available_tools": &*available_tools,
                        }));
                        continue;
                    };

                    let invocation_id = format!("graph-tool-{}-{}", ctx.step, idx);
                    let tool_ctx = Arc::new(ToolInvocationContext::new(
                        invocation_id,
                        user_id.clone(),
                        session_id.clone(),
                    )) as Arc<dyn ToolContext>;

                    match tool.execute(tool_ctx, call_arguments).await {
                        Ok(output) => {
                            results.push(json!({
                                "tool": called_tool_name,
                                "status": "ok",
                                "result": output,
                            }));
                        }
                        Err(err) => {
                            results.push(json!({
                                "tool": called_tool_name,
                                "status": "error",
                                "message": err.to_string(),
                            }));
                        }
                    }
                }

                let detail = format!("executed {} tool call(s)", results.len());
                Ok(NodeOutput::new()
                    .with_update(GRAPH_PENDING_TOOL_CALLS_KEY, json!([]))
                    .with_update(GRAPH_TOOL_RESULTS_KEY, Value::Array(results.clone()))
                    .with_update(GRAPH_TOOLS_EXECUTED_COUNT_KEY, json!(results.len()))
                    .with_update(
                        GRAPH_TRACE_KEY,
                        append_graph_trace(
                            &ctx.state,
                            GraphExecutionEvent {
                                step: ctx.step,
                                node: "tools".to_string(),
                                action: "node_end".to_string(),
                                detail,
                            },
                        ),
                    ))
            }
        })
        .edge(START, "resolve_tool_call")
        .conditional_edge(
            "resolve_tool_call",
            |state: &State| {
                if state
                    .get(GRAPH_PENDING_TOOL_CALLS_KEY)
                    .and_then(Value::as_array)
                    .is_some_and(|calls| !calls.is_empty())
                {
                    "tools".to_string()
                } else {
                    END.to_string()
                }
            },
            [("tools", "tools"), ("__end__", END)],
        )
        .edge("tools", END)
        .checkpointer(MemoryCheckpointer::new())
        .recursion_limit(recursion_limit.max(1));

    if role == "supervised" {
        builder = builder.interrupt_before(&["tools"]);
    }

    let graph = builder
        .build()
        .map_err(|e| anyhow::anyhow!("Failed to build graph tool runtime: {}", e))?;

    let mut input = State::new();
    input.insert(
        GRAPH_TOOL_REQUEST_KEY.to_string(),
        json!({ "name": tool_name, "arguments": arguments }),
    );
    input.insert(GRAPH_TRACE_KEY.to_string(), json!([]));

    let execution = GraphExecutionConfig::new(&thread_id).with_recursion_limit(recursion_limit);
    match graph.invoke(input, execution).await {
        Ok(state) => {
            let result = state
                .get(GRAPH_TOOL_RESULTS_KEY)
                .and_then(Value::as_array)
                .and_then(|items| items.first())
                .cloned()
                .unwrap_or_else(|| {
                    json!({
                        "tool": tool_name,
                        "status": "error",
                        "message": "Graph execution produced no tool result",
                    })
                });
            let report = GraphExecutionReport {
                thread_id,
                completed: true,
                interrupted: false,
                events: graph_trace_from_state(&state),
                final_state: graph_state_to_json(state),
            };
            Ok((result, report))
        }
        Err(GraphError::Interrupted(interrupt)) => {
            let mut events = graph_trace_from_state(&interrupt.state);
            events.push(GraphExecutionEvent {
                step: interrupt.step,
                node: "tools".to_string(),
                action: "interrupted".to_string(),
                detail: "Execution paused before tools node (supervised role)".to_string(),
            });
            let result = json!({
                "tool": tool_name,
                "status": "interrupted",
                "message": "Graph interrupted before tool execution. Resume flow is not implemented yet.",
            });
            let report = GraphExecutionReport {
                thread_id,
                completed: false,
                interrupted: true,
                events,
                final_state: graph_state_to_json(interrupt.state),
            };
            Ok((result, report))
        }
        Err(e) => Err(anyhow::anyhow!("Graph tool execution failed: {}", e)),
    }
}

fn compact_memory_entries(
    entries: &mut Vec<adk_memory::MemoryEntry>,
    max_events_threshold: usize,
) -> bool {
    if max_events_threshold < 2 || entries.len() <= max_events_threshold {
        return false;
    }

    let keep_recent = std::cmp::max(1, max_events_threshold / 2);
    let compact_count = entries.len().saturating_sub(keep_recent);
    if compact_count == 0 {
        return false;
    }

    let mut summary_chunks = Vec::new();
    for entry in entries.iter().take(compact_count) {
        if let Some(text) = entry
            .content
            .parts
            .iter()
            .find_map(|part| part.text().map(str::trim))
            .filter(|text| !text.is_empty())
        {
            summary_chunks.push(text.to_string());
        }
    }

    let mut summary_text = if summary_chunks.is_empty() {
        format!("compaction_summary: compacted {} events", compact_count)
    } else {
        format!("compaction_summary: {}", summary_chunks.join(" "))
    };
    if summary_text.len() > 4_096 {
        summary_text.truncate(4_096);
    }

    let summary_entry = adk_memory::MemoryEntry {
        content: Content::new("system").with_text(summary_text),
        author: "system".to_string(),
        timestamp: chrono::Utc::now(),
    };

    let mut compacted_entries = Vec::with_capacity(1 + keep_recent);
    compacted_entries.push(summary_entry);
    compacted_entries.extend(entries.iter().skip(compact_count).cloned());
    *entries = compacted_entries;
    true
}

async fn reconnect_with_backoff(
    session_id: &str,
    runtime: Arc<dyn SessionRuntime>,
    shutdown: Arc<AtomicBool>,
    diagnostics: Arc<RuntimeDiagnosticsCounters>,
    policy: &ReconnectPolicy,
) -> bool {
    if !policy.enable_reconnect || policy.max_attempts == 0 {
        return false;
    }

    for attempt in 1..=policy.max_attempts {
        if shutdown.load(Ordering::SeqCst) {
            return false;
        }
        diagnostics
            .reconnect_attempts_total
            .fetch_add(1, Ordering::SeqCst);

        let backoff = policy.backoff_for_attempt(attempt);
        warn!(
            session_id = session_id,
            attempt = attempt,
            max_attempts = policy.max_attempts,
            backoff_ms = backoff.as_millis(),
            "Attempting realtime reconnect"
        );
        tokio::time::sleep(backoff).await;

        if shutdown.load(Ordering::SeqCst) {
            return false;
        }

        match runtime.connect().await {
            Ok(()) => {
                diagnostics
                    .reconnect_successes_total
                    .fetch_add(1, Ordering::SeqCst);
                info!(
                    session_id = session_id,
                    attempt = attempt,
                    "Realtime reconnect succeeded"
                );
                return true;
            }
            Err(e) => {
                diagnostics
                    .reconnect_failures_total
                    .fetch_add(1, Ordering::SeqCst);
                warn!(
                    session_id = session_id,
                    attempt = attempt,
                    max_attempts = policy.max_attempts,
                    error = %e,
                    "Realtime reconnect attempt failed"
                );
            }
        }
    }

    warn!(
        session_id = session_id,
        max_attempts = policy.max_attempts,
        "Realtime reconnect budget exhausted"
    );
    false
}

fn spawn_runtime_loop(
    session_id: String,
    runtime: Arc<dyn SessionRuntime>,
    shutdown: Arc<AtomicBool>,
    diagnostics: Arc<RuntimeDiagnosticsCounters>,
    reconnect_policy: ReconnectPolicy,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            if shutdown.load(Ordering::SeqCst) {
                break;
            }

            match runtime.run().await {
                Ok(()) => {
                    info!(session_id = %session_id, "Realtime runner loop closed");
                    break;
                }
                Err(e) => {
                    warn!(
                        session_id = %session_id,
                        error = %e,
                        "Realtime runner loop ended with error"
                    );

                    let recovered = reconnect_with_backoff(
                        &session_id,
                        runtime.clone(),
                        shutdown.clone(),
                        diagnostics.clone(),
                        &reconnect_policy,
                    )
                    .await;
                    if !recovered {
                        break;
                    }
                }
            }
        }
    })
}

/// Event handler that forwards realtime outputs with the owning session ID.
struct GatewayEventForwarder {
    session_id: String,
    user_id: String,
    memory_store: Arc<dyn MemoryService>,
    session_memory_entries: Arc<RwLock<HashMap<String, Vec<adk_memory::MemoryEntry>>>>,
    artifact_service: Option<Arc<dyn ArtifactService>>,
    memory_compaction: MemoryCompactionPolicy,
    runtime_diagnostics: Arc<RuntimeDiagnosticsCounters>,
    audio_sequence: AtomicU64,
    transcript_sequence: AtomicU64,
    audio_tx: mpsc::Sender<SessionAudioOutput>,
    transcript_tx: mpsc::Sender<SessionTranscriptOutput>,
}

impl GatewayEventForwarder {
    async fn persist_transcript_memory(&self, transcript: &str) {
        let entry = adk_memory::MemoryEntry {
            content: Content::new("assistant").with_text(transcript),
            author: "assistant".to_string(),
            timestamp: chrono::Utc::now(),
        };

        let session_entries = {
            let mut guard = self.session_memory_entries.write().await;
            let entries = guard.entry(self.session_id.clone()).or_default();
            entries.push(entry);

            if self.memory_compaction.enabled
                && compact_memory_entries(entries, self.memory_compaction.max_events_threshold)
            {
                self.runtime_diagnostics
                    .compactions_applied_total
                    .fetch_add(1, Ordering::SeqCst);
                info!(
                    session_id = %self.session_id,
                    threshold = self.memory_compaction.max_events_threshold,
                    entry_count = entries.len(),
                    "Applied transcript memory compaction"
                );
            }
            entries.clone()
        };

        if let Err(e) = self
            .memory_store
            .add_session("liveclaw", &self.user_id, &self.session_id, session_entries)
            .await
        {
            warn!(
                session_id = %self.session_id,
                user_id = %self.user_id,
                error = %e,
                "Failed to persist transcript memory"
            );
        }
    }

    async fn persist_transcript_artifact(&self, transcript: &str) {
        let Some(service) = &self.artifact_service else {
            return;
        };
        let index = self.transcript_sequence.fetch_add(1, Ordering::SeqCst) + 1;
        let file_name = format!("transcript-{index:06}.txt");

        if let Err(e) = service
            .save(SaveRequest {
                app_name: "liveclaw".to_string(),
                user_id: self.user_id.clone(),
                session_id: self.session_id.clone(),
                file_name,
                part: Part::Text {
                    text: transcript.to_string(),
                },
                version: None,
            })
            .await
        {
            warn!(
                session_id = %self.session_id,
                user_id = %self.user_id,
                error = %e,
                "Failed to persist transcript artifact"
            );
        }
    }

    async fn persist_audio_artifact(&self, audio: &[u8]) {
        let Some(service) = &self.artifact_service else {
            return;
        };
        let index = self.audio_sequence.fetch_add(1, Ordering::SeqCst) + 1;
        let file_name = format!("audio-{index:06}.pcm");

        if let Err(e) = service
            .save(SaveRequest {
                app_name: "liveclaw".to_string(),
                user_id: self.user_id.clone(),
                session_id: self.session_id.clone(),
                file_name,
                part: Part::InlineData {
                    mime_type: "audio/pcm".to_string(),
                    data: audio.to_vec(),
                },
                version: None,
            })
            .await
        {
            warn!(
                session_id = %self.session_id,
                user_id = %self.user_id,
                error = %e,
                "Failed to persist audio artifact"
            );
        }
    }
}

#[async_trait]
impl EventHandler for GatewayEventForwarder {
    async fn on_audio(&self, audio: &[u8], _item_id: &str) -> adk_realtime::Result<()> {
        if self
            .audio_tx
            .send(SessionAudioOutput {
                session_id: self.session_id.clone(),
                data: audio.to_vec(),
            })
            .await
            .is_err()
        {
            warn!(
                session_id = %self.session_id,
                "Gateway audio output channel is closed"
            );
        }
        self.persist_audio_artifact(audio).await;
        Ok(())
    }

    async fn on_transcript(&self, transcript: &str, _item_id: &str) -> adk_realtime::Result<()> {
        if self
            .transcript_tx
            .send(SessionTranscriptOutput {
                session_id: self.session_id.clone(),
                text: transcript.to_string(),
                is_final: false,
            })
            .await
            .is_err()
        {
            warn!(
                session_id = %self.session_id,
                "Gateway transcript output channel is closed"
            );
        }
        self.persist_transcript_memory(transcript).await;
        self.persist_transcript_artifact(transcript).await;
        Ok(())
    }
}

/// Adapter implementing gateway session operations on top of adk-realtime.
struct RunnerAdapter {
    runtime_kind: RuntimeKind,
    provider: ProviderSelection,
    default_voice: Option<String>,
    default_instructions: Option<String>,
    tool_security: ToolSecurityConfig,
    tool_metrics: Arc<ToolExecutionMetrics>,
    runtime_diagnostics: Arc<RuntimeDiagnosticsCounters>,
    memory_store: Arc<dyn MemoryService>,
    session_memory_entries: Arc<RwLock<HashMap<String, Vec<adk_memory::MemoryEntry>>>>,
    artifact_service: Option<Arc<dyn ArtifactService>>,
    reconnect_policy: ReconnectPolicy,
    memory_compaction_policy: MemoryCompactionPolicy,
    graph_default_enabled: bool,
    graph_recursion_limit: usize,
    sessions: Arc<RwLock<HashMap<String, RealtimeSession>>>,
    audio_tx: mpsc::Sender<SessionAudioOutput>,
    transcript_tx: mpsc::Sender<SessionTranscriptOutput>,
}

struct RunnerAdapterInit {
    runtime_kind: RuntimeKind,
    provider: ProviderSelection,
    runtime_diagnostics: Arc<RuntimeDiagnosticsCounters>,
    memory_store: Arc<dyn MemoryService>,
    artifact_service: Option<Arc<dyn ArtifactService>>,
    reconnect_policy: ReconnectPolicy,
    memory_compaction_policy: MemoryCompactionPolicy,
    graph_default_enabled: bool,
    graph_recursion_limit: usize,
    audio_tx: mpsc::Sender<SessionAudioOutput>,
    transcript_tx: mpsc::Sender<SessionTranscriptOutput>,
}

impl RunnerAdapter {
    fn new(
        voice_cfg: &VoiceConfig,
        security_cfg: &AppSecurityConfig,
        init: RunnerAdapterInit,
    ) -> Self {
        Self {
            runtime_kind: init.runtime_kind,
            provider: init.provider,
            default_voice: voice_cfg.voice.clone(),
            default_instructions: voice_cfg.instructions.clone(),
            tool_security: ToolSecurityConfig::from(security_cfg),
            tool_metrics: Arc::new(ToolExecutionMetrics::default()),
            runtime_diagnostics: init.runtime_diagnostics,
            memory_store: init.memory_store,
            session_memory_entries: Arc::new(RwLock::new(HashMap::new())),
            artifact_service: init.artifact_service,
            reconnect_policy: init.reconnect_policy,
            memory_compaction_policy: init.memory_compaction_policy,
            graph_default_enabled: init.graph_default_enabled,
            graph_recursion_limit: init.graph_recursion_limit.max(1),
            sessions: Arc::new(RwLock::new(HashMap::new())),
            audio_tx: init.audio_tx,
            transcript_tx: init.transcript_tx,
        }
    }

    fn resolve_session_settings(
        &self,
        cfg: Option<&SessionConfig>,
    ) -> (String, String, Option<String>) {
        let model = non_empty(cfg.and_then(|c| c.model.clone()))
            .or_else(|| non_empty(Some(self.provider.default_model.clone())))
            .unwrap_or_else(|| "gpt-4o-realtime-preview-2024-12-17".to_string());

        let voice = non_empty(cfg.and_then(|c| c.voice.clone()))
            .or_else(|| non_empty(self.default_voice.clone()))
            .unwrap_or_else(|| "alloy".to_string());

        let instructions = non_empty(cfg.and_then(|c| c.instructions.clone()))
            .or_else(|| non_empty(self.default_instructions.clone()));

        (model, voice, instructions)
    }

    fn resolve_role(&self, cfg: Option<&SessionConfig>) -> Result<String> {
        let role = non_empty(cfg.and_then(|c| c.role.clone()))
            .unwrap_or_else(|| self.tool_security.default_role.clone())
            .to_lowercase();

        match role.as_str() {
            "readonly" | "supervised" | "full" => Ok(role),
            _ => bail!(
                "Unsupported role '{}'. Expected one of: readonly, supervised, full.",
                role
            ),
        }
    }

    fn build_session_access_control(
        &self,
        principal_id: &str,
        role: &str,
    ) -> Result<AccessControl> {
        let readonly = Role::new("readonly");

        let mut supervised = Role::new("supervised");
        for tool_name in &self.tool_security.tool_allowlist {
            supervised = supervised.allow(Permission::Tool(tool_name.clone()));
        }

        let full = Role::new("full").allow(Permission::AllTools);

        AccessControl::builder()
            .role(readonly)
            .role(supervised)
            .role(full)
            .assign(principal_id, role)
            .build()
            .map_err(|e| anyhow::anyhow!("Failed to build session access control: {}", e))
    }

    fn build_protected_tools(&self, principal_id: &str, role: &str) -> Result<Vec<Arc<dyn Tool>>> {
        let access_control = self.build_session_access_control(principal_id, role)?;
        let audit_sink = FileAuditSink::new(&self.tool_security.audit_log_path)
            .map_err(|e| anyhow::anyhow!("Failed to create audit sink: {}", e))?;
        let middleware = AuthMiddleware::with_audit(access_control, audit_sink);
        let workspace_policy = WorkspaceToolPolicy {
            workspace_root: std::path::PathBuf::from(&self.tool_security.workspace_root),
            forbidden_paths: self.tool_security.forbidden_tool_paths.clone(),
            max_read_bytes: 16_384,
        };
        Ok(middleware.protect_all(build_baseline_tools_with_workspace(workspace_policy)))
    }

    async fn runtime_for_session(&self, session_id: &str) -> Result<Arc<dyn SessionRuntime>> {
        let runtime = {
            let sessions = self.sessions.read().await;
            sessions.get(session_id).map(|s| s.runtime.clone())
        };
        runtime.ok_or_else(|| anyhow::anyhow!("Session '{}' not found", session_id))
    }
}

#[async_trait]
impl RunnerHandle for RunnerAdapter {
    async fn create_session(
        &self,
        user_id: &str,
        session_id: &str,
        config: Option<SessionConfig>,
    ) -> Result<String> {
        if self.tool_security.deny_by_default_principal_allowlist
            && !self
                .tool_security
                .principal_allowlist
                .iter()
                .any(|allowed| allowed == user_id)
        {
            bail!(
                "Principal '{}' is not in security.principal_allowlist and deny-by-default mode is enabled",
                user_id
            );
        }

        if self.provider.api_key.trim().is_empty() {
            bail!("Missing API key. Set [voice].api_key or LIVECLAW_API_KEY / OPENAI_API_KEY.");
        }

        let (model_id, voice, instructions) = self.resolve_session_settings(config.as_ref());
        let role = self.resolve_role(config.as_ref())?;
        let graph_enabled = config
            .as_ref()
            .and_then(|cfg| cfg.enable_graph)
            .unwrap_or(self.graph_default_enabled);
        let graph_recursion_limit = self.graph_recursion_limit.max(1);
        let protected_tools = self.build_protected_tools(user_id, &role)?;
        info!(
            session_id = session_id,
            runtime_kind = ?self.runtime_kind,
            provider = %self.provider.provider_label,
            provider_profile = %self.provider.profile_name,
            role = %role,
            graph_enabled = graph_enabled,
            graph_recursion_limit = graph_recursion_limit,
            tool_count = protected_tools.len(),
            "Configured baseline toolset for realtime session"
        );

        // Build adk-realtime runner for this session.
        let mut model = OpenAIRealtimeModel::new(self.provider.api_key.clone(), model_id.clone());
        match self.provider.kind {
            ProviderKind::OpenAI => {
                if let Some(base_url) = &self.provider.base_url {
                    model = model.with_base_url(base_url.clone());
                }
            }
            ProviderKind::OpenAICompatible => {
                let base_url = self.provider.base_url.clone().ok_or_else(|| {
                    anyhow::anyhow!(
                        "Provider profile '{}' requires a non-empty WebSocket base_url",
                        self.provider.profile_name
                    )
                })?;
                model = model.with_base_url(base_url);
            }
        }
        let mut tool_names = protected_tools
            .iter()
            .map(|tool| tool.name().to_string())
            .collect::<Vec<_>>();
        tool_names.sort();
        let merged_instruction = if !tool_names.is_empty() {
            let tool_guidance = format!(
                "You can call these tools: {}. When the user asks for math, current time/date, or workspace file content, call the relevant tool before answering.",
                tool_names.join(", ")
            );
            match instructions {
                Some(instr) if !instr.trim().is_empty() => {
                    Some(format!("{}\n\n{}", instr.trim(), tool_guidance))
                }
                _ => Some(tool_guidance),
            }
        } else {
            instructions
        };

        let mut realtime_config = RealtimeConfig::default()
            .with_model(model_id)
            .with_text_and_audio()
            .with_server_vad()
            .with_voice(voice);
        if let Some(instr) = merged_instruction {
            realtime_config = realtime_config.with_instruction(instr);
        }

        let mut runner_builder = RealtimeRunner::builder()
            .model(Arc::new(model))
            .config(realtime_config)
            .event_handler(GatewayEventForwarder {
                session_id: session_id.to_string(),
                user_id: user_id.to_string(),
                memory_store: self.memory_store.clone(),
                session_memory_entries: self.session_memory_entries.clone(),
                artifact_service: self.artifact_service.clone(),
                memory_compaction: self.memory_compaction_policy.clone(),
                runtime_diagnostics: self.runtime_diagnostics.clone(),
                audio_sequence: AtomicU64::new(0),
                transcript_sequence: AtomicU64::new(0),
                audio_tx: self.audio_tx.clone(),
                transcript_tx: self.transcript_tx.clone(),
            });

        let mut session_tools = HashMap::new();
        for tool in &protected_tools {
            session_tools.insert(tool.name().to_string(), tool.clone());
        }

        for tool in protected_tools {
            let definition = ToolDefinition {
                name: tool.name().to_string(),
                description: Some(tool.enhanced_description()),
                parameters: tool.parameters_schema(),
            };
            let handler = AdkToolHandler {
                tool: tool.clone(),
                session_id: session_id.to_string(),
                user_id: user_id.to_string(),
                metrics: self.tool_metrics.clone(),
            };
            runner_builder = runner_builder.tool(definition, handler);
        }

        let runner = Arc::new(
            runner_builder
                .build()
                .map_err(|e| anyhow::anyhow!("Failed to build realtime runner: {}", e))?,
        );

        let runtime: Arc<dyn SessionRuntime> = Arc::new(RealtimeRunnerRuntime::new(runner));
        runtime.connect().await?;
        let shutdown = Arc::new(AtomicBool::new(false));
        let run_task = spawn_runtime_loop(
            session_id.to_string(),
            runtime.clone(),
            shutdown.clone(),
            self.runtime_diagnostics.clone(),
            self.reconnect_policy.clone(),
        );

        let mut sessions = self.sessions.write().await;
        if sessions.contains_key(session_id) {
            let _ = runtime.close().await;
            run_task.abort();
            bail!("Session '{}' already exists", session_id);
        }

        sessions.insert(
            session_id.to_string(),
            RealtimeSession {
                runtime,
                run_task,
                user_id: user_id.to_string(),
                role,
                tools: session_tools,
                graph_enabled,
                graph_recursion_limit,
                shutdown,
            },
        );

        Ok(session_id.to_string())
    }

    async fn terminate_session(&self, session_id: &str) -> Result<()> {
        let session = { self.sessions.write().await.remove(session_id) };

        let Some(RealtimeSession {
            runtime,
            run_task,
            user_id,
            shutdown,
            ..
        }) = session
        else {
            bail!("Session '{}' not found", session_id);
        };

        shutdown.store(true, Ordering::SeqCst);

        if let Err(e) = runtime.close().await {
            warn!(session_id = session_id, error = %e, "Failed to close realtime session cleanly");
        }

        if let Some(mut entries) = self.session_memory_entries.write().await.remove(session_id) {
            let summary = entries
                .iter()
                .filter_map(|entry| {
                    entry
                        .content
                        .parts
                        .iter()
                        .filter_map(|part| part.text().map(str::to_string))
                        .next()
                })
                .collect::<Vec<_>>()
                .join(" ");

            if !summary.trim().is_empty() {
                entries.push(adk_memory::MemoryEntry {
                    content: Content::new("assistant")
                        .with_text(format!("session_summary: {}", summary)),
                    author: "system".to_string(),
                    timestamp: chrono::Utc::now(),
                });
            }

            if let Err(e) = self
                .memory_store
                .add_session("liveclaw", &user_id, session_id, entries)
                .await
            {
                warn!(
                    session_id = session_id,
                    user_id = %user_id,
                    error = %e,
                    "Failed to persist session memory summary on terminate"
                );
            }
        }

        run_task.abort();
        let _ = run_task.await;

        Ok(())
    }

    async fn send_audio(&self, session_id: &str, audio: &[u8]) -> Result<()> {
        if audio.is_empty() {
            bail!("Audio payload is empty");
        }

        let runtime = self.runtime_for_session(session_id).await?;

        let audio_b64 = base64_encode(audio);
        runtime.send_audio_base64(&audio_b64).await
    }

    async fn send_text(&self, session_id: &str, prompt: &str) -> Result<()> {
        let trimmed = prompt.trim();
        if trimmed.is_empty() {
            bail!("Prompt is empty");
        }
        let runtime = self.runtime_for_session(session_id).await?;
        runtime.send_text(trimmed).await
    }

    async fn commit_audio(&self, session_id: &str) -> Result<()> {
        let runtime = self.runtime_for_session(session_id).await?;
        runtime.commit_audio().await
    }

    async fn create_response(&self, session_id: &str) -> Result<()> {
        let runtime = self.runtime_for_session(session_id).await?;
        runtime.create_response().await
    }

    async fn interrupt_response(&self, session_id: &str) -> Result<()> {
        let runtime = self.runtime_for_session(session_id).await?;
        runtime.interrupt_response().await
    }

    async fn session_tool_call(
        &self,
        session_id: &str,
        tool_name: &str,
        arguments: Value,
    ) -> Result<(Value, GraphExecutionReport)> {
        let (user_id, role, tools, graph_enabled, graph_recursion_limit) = {
            let sessions = self.sessions.read().await;
            let Some(session) = sessions.get(session_id) else {
                bail!("Session '{}' not found", session_id);
            };
            (
                session.user_id.clone(),
                session.role.clone(),
                session.tools.clone(),
                session.graph_enabled,
                session.graph_recursion_limit,
            )
        };

        if !graph_enabled {
            bail!(
                "Graph execution is disabled for session '{}'. Create a session with enable_graph=true or set [graph].enable_graph=true.",
                session_id
            );
        }

        execute_session_tool_call_graph(
            session_id,
            &user_id,
            &role,
            tool_name,
            arguments,
            tools,
            graph_recursion_limit,
        )
        .await
    }

    async fn diagnostics(&self) -> Result<RuntimeDiagnostics> {
        let active_sessions = self.sessions.read().await.len();

        Ok(RuntimeDiagnostics {
            protocol_version: PROTOCOL_VERSION.to_string(),
            supported_client_messages: supported_client_message_types(),
            supported_server_responses: supported_server_response_types(),
            runtime_kind: runtime_kind_label(&self.runtime_kind).to_string(),
            provider_profile: self.provider.profile_name.clone(),
            provider_kind: provider_kind_label(&self.provider.kind).to_string(),
            provider_model: self.provider.default_model.clone(),
            provider_base_url: self.provider.base_url.clone(),
            reconnect_enabled: self.reconnect_policy.enable_reconnect,
            reconnect_max_attempts: self.reconnect_policy.max_attempts,
            reconnect_attempts_total: self
                .runtime_diagnostics
                .reconnect_attempts_total
                .load(Ordering::SeqCst),
            reconnect_successes_total: self
                .runtime_diagnostics
                .reconnect_successes_total
                .load(Ordering::SeqCst),
            reconnect_failures_total: self
                .runtime_diagnostics
                .reconnect_failures_total
                .load(Ordering::SeqCst),
            compaction_enabled: self.memory_compaction_policy.enabled,
            compaction_max_events_threshold: self.memory_compaction_policy.max_events_threshold,
            compactions_applied_total: self
                .runtime_diagnostics
                .compactions_applied_total
                .load(Ordering::SeqCst),
            security_workspace_root: self.tool_security.workspace_root.clone(),
            security_forbidden_tool_paths: self.tool_security.forbidden_tool_paths.clone(),
            security_deny_by_default_principal_allowlist: self
                .tool_security
                .deny_by_default_principal_allowlist,
            security_principal_allowlist_size: self.tool_security.principal_allowlist.len(),
            security_allow_public_bind: self.tool_security.allow_public_bind,
            active_sessions,
        })
    }
}

fn non_empty(value: Option<String>) -> Option<String> {
    value.and_then(|v| {
        let trimmed = v.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    })
}

fn effective_api_key(configured: &str) -> String {
    if !configured.trim().is_empty() {
        return configured.trim().to_string();
    }

    if let Ok(val) = std::env::var("LIVECLAW_API_KEY") {
        if !val.trim().is_empty() {
            return val.trim().to_string();
        }
    }

    std::env::var("OPENAI_API_KEY")
        .ok()
        .map(|v| v.trim().to_string())
        .unwrap_or_default()
}

enum AppCommand {
    Run { config_path: String },
    Doctor { config_path: String },
}

#[derive(Debug, Default)]
struct DoctorReport {
    notes: Vec<String>,
    warnings: Vec<String>,
}

fn parse_command() -> AppCommand {
    let mut args = std::env::args().skip(1);
    match args.next() {
        Some(flag) if flag == "--doctor" => AppCommand::Doctor {
            config_path: args.next().unwrap_or_else(|| "liveclaw.toml".to_string()),
        },
        Some(config_path) => AppCommand::Run { config_path },
        None => AppCommand::Run {
            config_path: "liveclaw.toml".to_string(),
        },
    }
}

fn provider_kind_label(kind: &ProviderKind) -> &'static str {
    match kind {
        ProviderKind::OpenAI => "openai",
        ProviderKind::OpenAICompatible => "openai_compatible",
    }
}

fn runtime_kind_label(kind: &RuntimeKind) -> &'static str {
    match kind {
        RuntimeKind::Native => "native",
        RuntimeKind::Docker => "docker",
    }
}

fn validate_ws_base_url(base_url: &str, profile_name: &str) -> Result<()> {
    let normalized = base_url.trim();
    if !(normalized.starts_with("ws://") || normalized.starts_with("wss://")) {
        bail!(
            "Provider profile '{}' base_url must start with ws:// or wss:// (got '{}')",
            profile_name,
            base_url
        );
    }
    Ok(())
}

fn resolve_profile_model(profile: &ProviderProfileConfig, voice: &VoiceConfig) -> String {
    non_empty(Some(profile.model.clone()))
        .or_else(|| non_empty(Some(voice.model.clone())))
        .unwrap_or_else(|| "gpt-4o-realtime-preview-2024-12-17".to_string())
}

fn resolve_profile_api_key(profile: &ProviderProfileConfig, voice: &VoiceConfig) -> String {
    if let Some(key) = non_empty(Some(profile.api_key.clone())) {
        return key;
    }
    effective_api_key(&voice.api_key)
}

fn resolve_profile_base_url(
    profile: &ProviderProfileConfig,
    voice: &VoiceConfig,
) -> Option<String> {
    non_empty(profile.base_url.clone()).or_else(|| non_empty(voice.base_url.clone()))
}

fn resolve_provider_selection(
    voice: &VoiceConfig,
    providers: &ProvidersConfig,
) -> Result<ProviderSelection> {
    let profile = non_empty(Some(providers.active_profile.clone()))
        .unwrap_or_else(|| "legacy".to_string())
        .to_lowercase();

    match profile.as_str() {
        "legacy" => {
            let provider_value = non_empty(Some(voice.provider.clone()))
                .unwrap_or_else(|| "openai".to_string())
                .to_lowercase();
            let kind = match provider_value.as_str() {
                "openai" => ProviderKind::OpenAI,
                "openai_compatible" | "openai-compatible" => ProviderKind::OpenAICompatible,
                other => {
                    bail!(
                        "Unsupported voice provider '{}' for legacy profile. Use openai or openai_compatible.",
                        other
                    )
                }
            };
            Ok(ProviderSelection {
                kind,
                profile_name: "legacy".to_string(),
                provider_label: provider_value,
                api_key: effective_api_key(&voice.api_key),
                default_model: non_empty(Some(voice.model.clone()))
                    .unwrap_or_else(|| "gpt-4o-realtime-preview-2024-12-17".to_string()),
                base_url: non_empty(voice.base_url.clone()),
            })
        }
        "openai" => Ok(ProviderSelection {
            kind: ProviderKind::OpenAI,
            profile_name: "openai".to_string(),
            provider_label: "openai".to_string(),
            api_key: resolve_profile_api_key(&providers.openai, voice),
            default_model: resolve_profile_model(&providers.openai, voice),
            base_url: resolve_profile_base_url(&providers.openai, voice),
        }),
        "openai_compatible" => Ok(ProviderSelection {
            kind: ProviderKind::OpenAICompatible,
            profile_name: "openai_compatible".to_string(),
            provider_label: "openai_compatible".to_string(),
            api_key: resolve_profile_api_key(&providers.openai_compatible, voice),
            default_model: resolve_profile_model(&providers.openai_compatible, voice),
            base_url: resolve_profile_base_url(&providers.openai_compatible, voice),
        }),
        other => bail!(
            "Unsupported providers.active_profile '{}'. Use legacy, openai, or openai_compatible.",
            other
        ),
    }
}

fn validate_runtime_and_provider(
    config: &LiveClawConfig,
    require_api_key: bool,
) -> Result<(ProviderSelection, DoctorReport)> {
    let provider = resolve_provider_selection(&config.voice, &config.providers)?;
    let mut report = DoctorReport::default();

    report.notes.push(format!(
        "runtime.kind={}",
        match config.runtime.kind {
            RuntimeKind::Native => "native",
            RuntimeKind::Docker => "docker",
        }
    ));
    report.notes.push(format!(
        "provider.profile={} provider.kind={}",
        provider.profile_name,
        provider_kind_label(&provider.kind)
    ));
    report
        .notes
        .push(format!("provider.model={}", provider.default_model));
    report.notes.push(format!(
        "security.workspace_root={} security.forbidden_tool_paths={}",
        config.security.workspace_root,
        config.security.forbidden_tool_paths.join(",")
    ));
    report.notes.push(format!(
        "security.principal_allowlist_size={} deny_by_default={}",
        config.security.principal_allowlist.len(),
        config.security.deny_by_default_principal_allowlist
    ));

    if provider.api_key.trim().is_empty() {
        if require_api_key {
            bail!(
                "No API key resolved for provider profile '{}'. Set profile api_key, [voice].api_key, LIVECLAW_API_KEY, or OPENAI_API_KEY.",
                provider.profile_name
            );
        }
        report.warnings.push(format!(
            "No API key resolved for provider profile '{}' (doctor mode allows this).",
            provider.profile_name
        ));
    }

    if provider.default_model.trim().is_empty() {
        bail!(
            "No model resolved for provider profile '{}'. Set profile model or [voice].model.",
            provider.profile_name
        );
    }

    if let Some(base_url) = &provider.base_url {
        validate_ws_base_url(base_url, &provider.profile_name)?;
    }

    if matches!(provider.kind, ProviderKind::OpenAICompatible) && provider.base_url.is_none() {
        bail!(
            "Provider profile '{}' requires base_url for OpenAI-compatible endpoints.",
            provider.profile_name
        );
    }

    if matches!(config.runtime.kind, RuntimeKind::Docker) {
        if config.runtime.docker_image.trim().is_empty() {
            bail!("runtime.docker_image must be set when runtime.kind is docker");
        }
        report.warnings.push(
            "runtime.kind=docker currently uses embedded runtime compatibility mode (container bridge is planned in a later sprint)".to_string(),
        );
        report.notes.push(format!(
            "runtime.docker_image={}",
            config.runtime.docker_image
        ));
    }

    let host = config.gateway.host.trim();
    let public_bind = host == "0.0.0.0" || host == "::";
    if public_bind && !config.security.allow_public_bind {
        bail!(
            "Refusing public bind on host '{}' because security.allow_public_bind=false",
            config.gateway.host
        );
    }
    if public_bind && config.security.allow_public_bind {
        report.warnings.push(format!(
            "gateway is configured for public bind on '{}' (operator override enabled)",
            config.gateway.host
        ));
    }

    Ok((provider, report))
}

fn print_doctor_report(config_path: &str, provider: &ProviderSelection, report: &DoctorReport) {
    println!("LiveClaw doctor report for '{}'", config_path);
    println!("protocol: {}", PROTOCOL_VERSION);
    println!(
        "provider: profile={} kind={} model={} base_url={}",
        provider.profile_name,
        provider_kind_label(&provider.kind),
        provider.default_model,
        provider
            .base_url
            .clone()
            .unwrap_or_else(|| "<default>".to_string())
    );
    for note in &report.notes {
        println!("note: {}", note);
    }
    if report.warnings.is_empty() {
        println!("warnings: none");
    } else {
        for warning in &report.warnings {
            println!("warning: {}", warning);
        }
    }
}

fn base64_encode(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);

    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };
        let triple = (b0 << 16) | (b1 << 8) | b2;

        out.push(TABLE[((triple >> 18) & 0x3F) as usize] as char);
        out.push(TABLE[((triple >> 12) & 0x3F) as usize] as char);

        if chunk.len() > 1 {
            out.push(TABLE[((triple >> 6) & 0x3F) as usize] as char);
        } else {
            out.push('=');
        }

        if chunk.len() > 2 {
            out.push(TABLE[(triple & 0x3F) as usize] as char);
        } else {
            out.push('=');
        }
    }

    out
}

#[tokio::main]
async fn main() -> Result<()> {
    let command = parse_command();
    let config_path = match &command {
        AppCommand::Run { config_path } | AppCommand::Doctor { config_path } => config_path,
    };

    let config = LiveClawConfig::load(config_path)?;
    let require_api_key = matches!(command, AppCommand::Run { .. });
    let (provider, doctor_report) = validate_runtime_and_provider(&config, require_api_key)?;

    if matches!(command, AppCommand::Doctor { .. }) {
        print_doctor_report(config_path, &provider, &doctor_report);
        return Ok(());
    }

    // Initialize telemetry
    if config.telemetry.otlp_enabled {
        init_with_otlp("liveclaw", "http://localhost:4317").ok();
    } else {
        init_telemetry("liveclaw").ok();
    }
    info!("LiveClaw starting with config from '{}'", config_path);
    for note in &doctor_report.notes {
        info!(diagnostic = %note, "Configuration diagnostic");
    }
    for warning_note in &doctor_report.warnings {
        warn!(diagnostic = %warning_note, "Configuration warning");
    }

    // Gateway output channels consumed by background routing tasks.
    let (gw_audio_tx, gw_audio_rx) = mpsc::channel::<SessionAudioOutput>(256);
    let (gw_transcript_tx, gw_transcript_rx) = mpsc::channel::<SessionTranscriptOutput>(256);

    let pairing = Arc::new(PairingGuard::with_lockout(
        config.gateway.require_pairing,
        &[],
        config.pairing.max_attempts,
        Duration::from_secs(config.pairing.lockout_duration_secs),
    ));

    let gateway_config = GatewayConfig {
        host: config.gateway.host.clone(),
        port: config.gateway.port,
    };

    let memory_store = build_memory_service(&config.memory.backend).with_context(|| {
        format!(
            "Failed to initialize memory backend '{}'",
            config.memory.backend
        )
    })?;
    let artifact_service: Option<Arc<dyn ArtifactService>> = if config.artifact.enable_artifacts {
        Some(Arc::new(
            FileArtifactService::new(&config.artifact.storage_path).with_context(|| {
                format!(
                    "Failed to initialize artifact storage '{}'",
                    config.artifact.storage_path
                )
            })?,
        ))
    } else {
        None
    };

    let runner_handle: Arc<dyn RunnerHandle> = Arc::new(RunnerAdapter::new(
        &config.voice,
        &config.security,
        RunnerAdapterInit {
            runtime_kind: config.runtime.kind.clone(),
            provider,
            runtime_diagnostics: Arc::new(RuntimeDiagnosticsCounters::default()),
            memory_store,
            artifact_service,
            reconnect_policy: ReconnectPolicy::from(&config.resilience),
            memory_compaction_policy: MemoryCompactionPolicy::from(&config.compaction),
            graph_default_enabled: config.graph.enable_graph,
            graph_recursion_limit: config.graph.recursion_limit,
            audio_tx: gw_audio_tx,
            transcript_tx: gw_transcript_tx,
        },
    ));

    let gateway = Gateway::with_output_channels(
        gateway_config,
        pairing,
        runner_handle,
        gw_audio_rx,
        gw_transcript_rx,
    );

    info!(
        "Starting Gateway on {}:{}",
        config.gateway.host, config.gateway.port
    );

    gateway.start().await
}

#[cfg(test)]
mod tests {
    use super::*;
    use adk_artifact::ListRequest;
    use adk_memory::SearchRequest;
    use liveclaw_app::config::SecurityConfig as AppSecurityConfig;
    use liveclaw_app::storage::FileArtifactService;
    use std::collections::VecDeque;
    use std::sync::atomic::AtomicUsize;

    fn test_adapter_with_workspace(
        default_role: &str,
        allowlist: Vec<&str>,
        workspace_root: &std::path::Path,
    ) -> RunnerAdapter {
        let voice_cfg = VoiceConfig {
            provider: "openai".to_string(),
            api_key: "test-key".to_string(),
            model: "gpt-4o-realtime-preview-2024-12-17".to_string(),
            base_url: None,
            voice: Some("alloy".to_string()),
            instructions: Some("test".to_string()),
            audio_format: liveclaw_app::config::AudioFormat::Pcm16_24kHz,
        };

        let security_cfg = AppSecurityConfig {
            default_role: default_role.to_string(),
            tool_allowlist: allowlist.into_iter().map(str::to_string).collect(),
            rate_limit_per_session: 100,
            audit_log_path: "/tmp/liveclaw-test-audit.jsonl".to_string(),
            workspace_root: workspace_root.to_string_lossy().to_string(),
            forbidden_tool_paths: vec![".git".to_string(), "target".to_string()],
            principal_allowlist: Vec::new(),
            deny_by_default_principal_allowlist: false,
            allow_public_bind: false,
        };

        let (audio_tx, _audio_rx) = mpsc::channel(4);
        let (transcript_tx, _transcript_rx) = mpsc::channel(4);
        let memory_store: Arc<dyn MemoryService> =
            Arc::new(adk_memory::InMemoryMemoryService::new());

        RunnerAdapter::new(
            &voice_cfg,
            &security_cfg,
            RunnerAdapterInit {
                runtime_kind: RuntimeKind::Native,
                provider: ProviderSelection {
                    kind: ProviderKind::OpenAI,
                    profile_name: "legacy".to_string(),
                    provider_label: "openai".to_string(),
                    api_key: "test-key".to_string(),
                    default_model: "gpt-4o-realtime-preview-2024-12-17".to_string(),
                    base_url: None,
                },
                runtime_diagnostics: Arc::new(RuntimeDiagnosticsCounters::default()),
                memory_store,
                artifact_service: None,
                reconnect_policy: ReconnectPolicy::from(&ResilienceConfig::default()),
                memory_compaction_policy: MemoryCompactionPolicy::from(&CompactionConfig::default()),
                graph_default_enabled: true,
                graph_recursion_limit: 25,
                audio_tx,
                transcript_tx,
            },
        )
    }

    fn test_adapter(default_role: &str, allowlist: Vec<&str>) -> RunnerAdapter {
        test_adapter_with_workspace(default_role, allowlist, std::path::Path::new("."))
    }

    #[test]
    fn test_base64_encode_known_values() {
        assert_eq!(base64_encode(&[1, 2, 3]), "AQID");
        assert_eq!(base64_encode(b"a"), "YQ==");
        assert_eq!(base64_encode(&[]), "");
    }

    #[tokio::test]
    async fn test_gateway_event_forwarder_tags_audio_and_transcript() {
        let (audio_tx, mut audio_rx) = mpsc::channel(4);
        let (transcript_tx, mut transcript_rx) = mpsc::channel(4);
        let memory_store: Arc<dyn MemoryService> =
            Arc::new(adk_memory::InMemoryMemoryService::new());

        let handler = GatewayEventForwarder {
            session_id: "sess-42".to_string(),
            user_id: "user-42".to_string(),
            memory_store,
            session_memory_entries: Arc::new(RwLock::new(HashMap::new())),
            artifact_service: None,
            memory_compaction: MemoryCompactionPolicy {
                enabled: false,
                max_events_threshold: 500,
            },
            runtime_diagnostics: Arc::new(RuntimeDiagnosticsCounters::default()),
            audio_sequence: AtomicU64::new(0),
            transcript_sequence: AtomicU64::new(0),
            audio_tx,
            transcript_tx,
        };

        handler.on_audio(&[7, 8, 9], "item-a").await.unwrap();
        handler.on_transcript("hello", "item-b").await.unwrap();

        let audio = audio_rx.recv().await.unwrap();
        assert_eq!(audio.session_id, "sess-42");
        assert_eq!(audio.data, vec![7, 8, 9]);

        let transcript = transcript_rx.recv().await.unwrap();
        assert_eq!(transcript.session_id, "sess-42");
        assert_eq!(transcript.text, "hello");
        assert!(!transcript.is_final);
    }

    #[tokio::test]
    async fn test_gateway_event_forwarder_persists_memory_and_artifacts() {
        let (audio_tx, mut audio_rx) = mpsc::channel(4);
        let (transcript_tx, mut transcript_rx) = mpsc::channel(4);

        let artifact_root = std::env::temp_dir().join(format!(
            "liveclaw-artifacts-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let memory_file = std::env::temp_dir().join(format!(
            "liveclaw-memory-{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));

        let memory_store =
            build_memory_service(&format!("file:{}", memory_file.display())).unwrap();
        let artifact_service: Arc<dyn ArtifactService> =
            Arc::new(FileArtifactService::new(&artifact_root).unwrap());

        let handler = GatewayEventForwarder {
            session_id: "sess-art-1".to_string(),
            user_id: "user-art-1".to_string(),
            memory_store: memory_store.clone(),
            session_memory_entries: Arc::new(RwLock::new(HashMap::new())),
            artifact_service: Some(artifact_service.clone()),
            memory_compaction: MemoryCompactionPolicy {
                enabled: false,
                max_events_threshold: 500,
            },
            runtime_diagnostics: Arc::new(RuntimeDiagnosticsCounters::default()),
            audio_sequence: AtomicU64::new(0),
            transcript_sequence: AtomicU64::new(0),
            audio_tx,
            transcript_tx,
        };

        handler.on_audio(&[7, 8, 9], "item-a").await.unwrap();
        handler
            .on_transcript("favorite_color is blue", "item-b")
            .await
            .unwrap();

        let _ = audio_rx.recv().await.unwrap();
        let _ = transcript_rx.recv().await.unwrap();

        let memories = memory_store
            .search(SearchRequest {
                query: "favorite_color".to_string(),
                user_id: "user-art-1".to_string(),
                app_name: "liveclaw".to_string(),
            })
            .await
            .unwrap()
            .memories;
        assert!(!memories.is_empty(), "expected persisted memory entries");

        let listed = artifact_service
            .list(ListRequest {
                app_name: "liveclaw".to_string(),
                user_id: "user-art-1".to_string(),
                session_id: "sess-art-1".to_string(),
            })
            .await
            .unwrap()
            .file_names;
        assert!(
            listed.iter().any(|name| name.starts_with("transcript-")),
            "expected transcript artifacts in {:?}",
            listed
        );
        assert!(
            listed.iter().any(|name| name.starts_with("audio-")),
            "expected audio artifacts in {:?}",
            listed
        );

        let _ = std::fs::remove_file(memory_file);
        let _ = std::fs::remove_dir_all(artifact_root);
    }

    #[test]
    fn test_protected_tools_catalog_is_non_empty() {
        let adapter = test_adapter(
            "supervised",
            vec![
                "echo_text",
                "add_numbers",
                "utc_time",
                "read_workspace_file",
            ],
        );
        let tools = adapter
            .build_protected_tools("principal-1", "supervised")
            .unwrap();
        assert_eq!(tools.len(), 4);
    }

    #[tokio::test]
    async fn test_tool_handler_denies_readonly_role() {
        let adapter = test_adapter("readonly", vec!["echo_text"]);
        let tool = adapter
            .build_protected_tools("principal-1", "readonly")
            .unwrap()
            .into_iter()
            .find(|t| t.name() == "echo_text")
            .unwrap();

        let handler = AdkToolHandler {
            tool,
            session_id: "sess-1".to_string(),
            user_id: "principal-1".to_string(),
            metrics: Arc::new(ToolExecutionMetrics::default()),
        };

        let call = ToolCall {
            call_id: "call-1".to_string(),
            name: "echo_text".to_string(),
            arguments: serde_json::json!({ "text": "hello" }),
        };

        let err = handler.execute(&call).await.err().unwrap();
        assert!(matches!(err, RealtimeError::ToolError(_)));
    }

    #[tokio::test]
    async fn test_tool_handler_executes_for_full_role_and_tracks_metrics() {
        let adapter = test_adapter("full", vec![]);
        let tool = adapter
            .build_protected_tools("principal-1", "full")
            .unwrap()
            .into_iter()
            .find(|t| t.name() == "echo_text")
            .unwrap();

        let metrics = Arc::new(ToolExecutionMetrics::default());
        let handler = AdkToolHandler {
            tool,
            session_id: "sess-1".to_string(),
            user_id: "principal-1".to_string(),
            metrics: metrics.clone(),
        };

        let call = ToolCall {
            call_id: "call-1".to_string(),
            name: "echo_text".to_string(),
            arguments: serde_json::json!({ "text": "hello" }),
        };

        let result = handler.execute(&call).await.unwrap();
        assert_eq!(result["text"], "hello");
        assert_eq!(result["length"], 5);

        let (total, failed, _duration) = metrics.snapshot();
        assert_eq!(total, 1);
        assert_eq!(failed, 0);
    }

    #[tokio::test]
    async fn test_execute_session_tool_call_graph_runs_tool_and_records_trace() {
        let adapter = test_adapter("full", vec![]);
        let tools = adapter
            .build_protected_tools("principal-1", "full")
            .unwrap()
            .into_iter()
            .map(|tool| (tool.name().to_string(), tool))
            .collect::<HashMap<_, _>>();

        let (result, report) = execute_session_tool_call_graph(
            "sess-graph-1",
            "principal-1",
            "full",
            "echo_text",
            json!({ "text": "hello graph" }),
            tools,
            25,
        )
        .await
        .unwrap();

        assert_eq!(result["status"], json!("ok"));
        assert_eq!(result["tool"], json!("echo_text"));
        assert_eq!(result["result"]["text"], json!("hello graph"));
        assert!(report.completed);
        assert!(!report.interrupted);
        assert!(
            report
                .events
                .iter()
                .any(|event| event.node == "resolve_tool_call"),
            "expected resolve_tool_call in trace events: {:?}",
            report.events
        );
        assert!(
            report.events.iter().any(|event| event.node == "tools"),
            "expected tools in trace events: {:?}",
            report.events
        );
    }

    #[tokio::test]
    async fn test_execute_session_tool_call_graph_supervised_interrupts_before_tools() {
        let adapter = test_adapter("supervised", vec!["echo_text"]);
        let tools = adapter
            .build_protected_tools("principal-1", "supervised")
            .unwrap()
            .into_iter()
            .map(|tool| (tool.name().to_string(), tool))
            .collect::<HashMap<_, _>>();

        let (result, report) = execute_session_tool_call_graph(
            "sess-graph-2",
            "principal-1",
            "supervised",
            "echo_text",
            json!({ "text": "needs approval" }),
            tools,
            25,
        )
        .await
        .unwrap();

        assert_eq!(result["status"], json!("interrupted"));
        assert!(report.interrupted);
        assert!(!report.completed);
    }

    #[tokio::test]
    async fn test_execute_session_tool_call_graph_reads_workspace_file_from_prompt_payload() {
        let workspace = std::env::temp_dir().join(format!(
            "liveclaw-prompt-read-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let target_file = workspace.join("notes").join("prompt.txt");
        std::fs::create_dir_all(target_file.parent().unwrap()).unwrap();
        std::fs::write(&target_file, "tool prompt read success").unwrap();

        let adapter = test_adapter_with_workspace("full", vec![], &workspace);
        let tools = adapter
            .build_protected_tools("principal-1", "full")
            .unwrap()
            .into_iter()
            .map(|tool| (tool.name().to_string(), tool))
            .collect::<HashMap<_, _>>();

        let (result, report) = execute_session_tool_call_graph(
            "sess-graph-read-1",
            "principal-1",
            "full",
            "read_workspace_file",
            json!({
                "path": "notes/prompt.txt",
                "max_bytes": 256,
                "prompt": "Use read_workspace_file to load notes/prompt.txt"
            }),
            tools,
            25,
        )
        .await
        .unwrap();

        assert_eq!(result["status"], json!("ok"));
        assert_eq!(result["tool"], json!("read_workspace_file"));
        assert_eq!(result["result"]["path"], json!("notes/prompt.txt"));
        assert_eq!(result["result"]["truncated"], json!(false));
        let content = result["result"]["content"].as_str().unwrap_or_default();
        assert!(
            content.contains("tool prompt read success"),
            "unexpected read content: {}",
            content
        );
        assert!(report.completed);
        assert!(!report.interrupted);
        assert!(
            report.events.iter().any(|event| event.node == "tools"),
            "expected tools node execution in graph trace"
        );

        let _ = std::fs::remove_dir_all(workspace);
    }

    #[test]
    fn test_reconnect_policy_backoff_caps_at_max() {
        let policy = ReconnectPolicy {
            enable_reconnect: true,
            max_attempts: 5,
            initial_backoff: Duration::from_millis(50),
            max_backoff: Duration::from_millis(300),
        };

        assert_eq!(policy.backoff_for_attempt(1), Duration::from_millis(50));
        assert_eq!(policy.backoff_for_attempt(2), Duration::from_millis(100));
        assert_eq!(policy.backoff_for_attempt(3), Duration::from_millis(200));
        assert_eq!(policy.backoff_for_attempt(4), Duration::from_millis(300));
        assert_eq!(policy.backoff_for_attempt(5), Duration::from_millis(300));
    }

    fn memory_entry(text: &str) -> adk_memory::MemoryEntry {
        adk_memory::MemoryEntry {
            content: Content::new("assistant").with_text(text),
            author: "assistant".to_string(),
            timestamp: chrono::Utc::now(),
        }
    }

    #[test]
    fn test_compact_memory_entries_reduces_history_when_threshold_exceeded() {
        let mut entries = vec![
            memory_entry("alpha one"),
            memory_entry("beta two"),
            memory_entry("gamma three"),
            memory_entry("delta four"),
            memory_entry("epsilon five"),
            memory_entry("zeta six"),
        ];

        let compacted = compact_memory_entries(&mut entries, 4);
        assert!(compacted);
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].author, "system");
        let summary = entries[0]
            .content
            .parts
            .iter()
            .find_map(Part::text)
            .unwrap_or_default();
        assert!(summary.contains("compaction_summary"));
        assert!(summary.contains("alpha one"));
    }

    #[derive(Clone, Copy)]
    enum ScriptedResult {
        Ok,
        Err(&'static str),
    }

    #[derive(Default)]
    struct MockSessionRuntime {
        run_results: Mutex<VecDeque<ScriptedResult>>,
        connect_results: Mutex<VecDeque<ScriptedResult>>,
        close_calls: AtomicUsize,
        run_calls: AtomicUsize,
        connect_calls: AtomicUsize,
    }

    impl MockSessionRuntime {
        fn with_scripts(
            run_results: Vec<ScriptedResult>,
            connect_results: Vec<ScriptedResult>,
        ) -> Self {
            Self {
                run_results: Mutex::new(VecDeque::from(run_results)),
                connect_results: Mutex::new(VecDeque::from(connect_results)),
                close_calls: AtomicUsize::new(0),
                run_calls: AtomicUsize::new(0),
                connect_calls: AtomicUsize::new(0),
            }
        }
    }

    #[async_trait::async_trait]
    impl SessionRuntime for MockSessionRuntime {
        async fn connect(&self) -> Result<()> {
            self.connect_calls.fetch_add(1, Ordering::SeqCst);
            let outcome = self
                .connect_results
                .lock()
                .expect("connect script lock poisoned")
                .pop_front()
                .unwrap_or(ScriptedResult::Ok);
            match outcome {
                ScriptedResult::Ok => Ok(()),
                ScriptedResult::Err(msg) => Err(anyhow::anyhow!(msg)),
            }
        }

        async fn run(&self) -> Result<()> {
            self.run_calls.fetch_add(1, Ordering::SeqCst);
            let outcome = self
                .run_results
                .lock()
                .expect("run script lock poisoned")
                .pop_front()
                .unwrap_or(ScriptedResult::Ok);
            match outcome {
                ScriptedResult::Ok => Ok(()),
                ScriptedResult::Err(msg) => Err(anyhow::anyhow!(msg)),
            }
        }

        async fn close(&self) -> Result<()> {
            self.close_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        async fn send_audio_base64(&self, _audio_base64: &str) -> Result<()> {
            Ok(())
        }

        async fn send_text(&self, _text: &str) -> Result<()> {
            Ok(())
        }

        async fn commit_audio(&self) -> Result<()> {
            Ok(())
        }

        async fn create_response(&self) -> Result<()> {
            Ok(())
        }

        async fn interrupt_response(&self) -> Result<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn test_runtime_loop_recovers_from_interrupted_provider_session() {
        let runtime = Arc::new(MockSessionRuntime::with_scripts(
            vec![ScriptedResult::Err("provider drop"), ScriptedResult::Ok],
            vec![ScriptedResult::Ok],
        ));
        let shutdown = Arc::new(AtomicBool::new(false));
        let policy = ReconnectPolicy {
            enable_reconnect: true,
            max_attempts: 3,
            initial_backoff: Duration::from_millis(1),
            max_backoff: Duration::from_millis(2),
        };

        let handle = spawn_runtime_loop(
            "sess-recover".to_string(),
            runtime.clone(),
            shutdown,
            Arc::new(RuntimeDiagnosticsCounters::default()),
            policy,
        );
        tokio::time::timeout(Duration::from_millis(200), handle)
            .await
            .expect("runtime loop should complete")
            .expect("runtime task should not panic");

        assert_eq!(runtime.run_calls.load(Ordering::SeqCst), 2);
        assert_eq!(runtime.connect_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_runtime_loop_stops_when_reconnect_budget_exhausted() {
        let runtime = Arc::new(MockSessionRuntime::with_scripts(
            vec![ScriptedResult::Err("provider drop")],
            vec![
                ScriptedResult::Err("connect fail 1"),
                ScriptedResult::Err("connect fail 2"),
            ],
        ));
        let shutdown = Arc::new(AtomicBool::new(false));
        let policy = ReconnectPolicy {
            enable_reconnect: true,
            max_attempts: 2,
            initial_backoff: Duration::from_millis(1),
            max_backoff: Duration::from_millis(1),
        };

        let handle = spawn_runtime_loop(
            "sess-budget".to_string(),
            runtime.clone(),
            shutdown,
            Arc::new(RuntimeDiagnosticsCounters::default()),
            policy,
        );
        tokio::time::timeout(Duration::from_millis(200), handle)
            .await
            .expect("runtime loop should complete")
            .expect("runtime task should not panic");

        assert_eq!(runtime.run_calls.load(Ordering::SeqCst), 1);
        assert_eq!(runtime.connect_calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn test_runner_diagnostics_snapshot_exposes_runtime_and_provider_config() {
        let adapter = test_adapter("full", vec![]);
        let diagnostics = adapter.diagnostics().await.unwrap();

        assert_eq!(diagnostics.runtime_kind, "native");
        assert_eq!(diagnostics.provider_profile, "legacy");
        assert_eq!(diagnostics.provider_kind, "openai");
        assert_eq!(diagnostics.protocol_version, PROTOCOL_VERSION);
        assert!(diagnostics
            .supported_client_messages
            .contains(&"GetDiagnostics".to_string()));
        assert!(diagnostics
            .supported_server_responses
            .contains(&"Diagnostics".to_string()));
        assert_eq!(
            diagnostics.provider_model,
            "gpt-4o-realtime-preview-2024-12-17"
        );
        assert_eq!(diagnostics.reconnect_max_attempts, 5);
        assert_eq!(diagnostics.compaction_max_events_threshold, 500);
        assert_eq!(diagnostics.security_workspace_root, ".");
        assert_eq!(
            diagnostics.security_forbidden_tool_paths,
            vec![".git".to_string(), "target".to_string()]
        );
        assert!(!diagnostics.security_deny_by_default_principal_allowlist);
        assert_eq!(diagnostics.security_principal_allowlist_size, 0);
        assert!(!diagnostics.security_allow_public_bind);
        assert_eq!(diagnostics.active_sessions, 0);
    }

    #[tokio::test]
    async fn test_create_session_denied_when_principal_not_allowlisted() {
        let mut adapter = test_adapter("full", vec![]);
        adapter.tool_security.deny_by_default_principal_allowlist = true;
        adapter.tool_security.principal_allowlist = vec!["principal-allowed".to_string()];

        let err = adapter
            .create_session("principal-denied", "sess-denied", None)
            .await
            .unwrap_err();
        assert!(err
            .to_string()
            .contains("not in security.principal_allowlist"));
    }

    #[test]
    fn test_resolve_provider_selection_legacy_openai() {
        let voice = VoiceConfig {
            provider: "openai".to_string(),
            api_key: "legacy-key".to_string(),
            model: "gpt-4o-realtime-preview-2024-12-17".to_string(),
            base_url: None,
            voice: Some("alloy".to_string()),
            instructions: Some("test".to_string()),
            audio_format: liveclaw_app::config::AudioFormat::Pcm16_24kHz,
        };
        let providers = ProvidersConfig::default();

        let resolved = resolve_provider_selection(&voice, &providers).unwrap();
        assert_eq!(resolved.profile_name, "legacy");
        assert_eq!(resolved.provider_label, "openai");
        assert_eq!(resolved.default_model, "gpt-4o-realtime-preview-2024-12-17");
        assert_eq!(resolved.api_key, "legacy-key");
        assert!(matches!(resolved.kind, ProviderKind::OpenAI));
    }

    #[test]
    fn test_resolve_provider_selection_openai_compatible_profile() {
        let voice = VoiceConfig {
            provider: "openai".to_string(),
            api_key: "fallback-key".to_string(),
            model: "fallback-model".to_string(),
            base_url: None,
            voice: Some("alloy".to_string()),
            instructions: Some("test".to_string()),
            audio_format: liveclaw_app::config::AudioFormat::Pcm16_24kHz,
        };
        let providers = ProvidersConfig {
            active_profile: "openai_compatible".to_string(),
            openai: ProviderProfileConfig::default(),
            openai_compatible: ProviderProfileConfig {
                enabled: true,
                api_key: "compat-key".to_string(),
                model: "gpt-4o-realtime-preview-2024-12-17".to_string(),
                base_url: Some("wss://realtime.proxy.local/v1/realtime".to_string()),
            },
        };

        let resolved = resolve_provider_selection(&voice, &providers).unwrap();
        assert_eq!(resolved.profile_name, "openai_compatible");
        assert_eq!(resolved.provider_label, "openai_compatible");
        assert_eq!(resolved.api_key, "compat-key");
        assert_eq!(
            resolved.base_url.as_deref(),
            Some("wss://realtime.proxy.local/v1/realtime")
        );
        assert!(matches!(resolved.kind, ProviderKind::OpenAICompatible));
    }

    #[test]
    fn test_validate_runtime_and_provider_rejects_missing_compat_base_url() {
        let cfg = LiveClawConfig {
            voice: VoiceConfig {
                provider: "openai".to_string(),
                api_key: "legacy-key".to_string(),
                model: "gpt-4o-realtime-preview-2024-12-17".to_string(),
                base_url: None,
                voice: Some("alloy".to_string()),
                instructions: Some("test".to_string()),
                audio_format: liveclaw_app::config::AudioFormat::Pcm16_24kHz,
            },
            providers: ProvidersConfig {
                active_profile: "openai_compatible".to_string(),
                openai: ProviderProfileConfig::default(),
                openai_compatible: ProviderProfileConfig {
                    enabled: true,
                    api_key: "compat-key".to_string(),
                    model: "gpt-4o-realtime-preview-2024-12-17".to_string(),
                    base_url: None,
                },
            },
            ..LiveClawConfig::default()
        };

        let err = validate_runtime_and_provider(&cfg, true).unwrap_err();
        assert!(err
            .to_string()
            .contains("requires base_url for OpenAI-compatible endpoints"));
    }

    #[test]
    fn test_validate_runtime_and_provider_rejects_non_ws_base_url() {
        let cfg = LiveClawConfig {
            voice: VoiceConfig {
                provider: "openai".to_string(),
                api_key: "legacy-key".to_string(),
                model: "gpt-4o-realtime-preview-2024-12-17".to_string(),
                base_url: None,
                voice: Some("alloy".to_string()),
                instructions: Some("test".to_string()),
                audio_format: liveclaw_app::config::AudioFormat::Pcm16_24kHz,
            },
            providers: ProvidersConfig {
                active_profile: "openai".to_string(),
                openai: ProviderProfileConfig {
                    enabled: true,
                    api_key: "openai-key".to_string(),
                    model: "gpt-4o-realtime-preview-2024-12-17".to_string(),
                    base_url: Some("https://api.openai.com/v1/realtime".to_string()),
                },
                openai_compatible: ProviderProfileConfig::default(),
            },
            ..LiveClawConfig::default()
        };

        let err = validate_runtime_and_provider(&cfg, true).unwrap_err();
        assert!(err.to_string().contains("must start with ws:// or wss://"));
    }

    #[test]
    fn test_validate_runtime_and_provider_rejects_public_bind_without_override() {
        let mut cfg = LiveClawConfig::default();
        cfg.gateway.host = "0.0.0.0".to_string();
        cfg.security.allow_public_bind = false;

        let err = validate_runtime_and_provider(&cfg, false).unwrap_err();
        assert!(err
            .to_string()
            .contains("Refusing public bind on host '0.0.0.0'"));
    }

    #[test]
    fn test_validate_runtime_and_provider_allows_public_bind_with_override_warning() {
        let mut cfg = LiveClawConfig::default();
        cfg.gateway.host = "0.0.0.0".to_string();
        cfg.security.allow_public_bind = true;

        let (_provider, report) = validate_runtime_and_provider(&cfg, false).unwrap();
        assert!(report
            .warnings
            .iter()
            .any(|warning| warning.contains("public bind")));
    }
}
