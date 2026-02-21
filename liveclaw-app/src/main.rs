//! LiveClaw entry point — wires ADK-Rust realtime sessions to the gateway.

use std::collections::HashMap;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command as StdCommand, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use base64::Engine;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{mpsc, oneshot, Mutex as AsyncMutex, RwLock};
use tokio::task::JoinHandle;
use tracing::{info, warn};

use adk_artifact::{ArtifactService, SaveRequest};
use adk_auth::{AccessControl, AuthMiddleware, FileAuditSink, Permission, Role};
use adk_core::{
    Agent, CallbackContext, Content, Event, EventActions, EventCompaction,
    MemoryEntry as CoreMemoryEntry, Part, ReadonlyContext, Tool, ToolContext,
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
use adk_runner::EventsCompactionConfig;
use adk_session::{CreateRequest, GetRequest, InMemorySessionService, SessionService};
use adk_telemetry::{init_telemetry, init_with_otlp};
use serde_json::{json, Value};

use liveclaw_app::config::{
    CompactionConfig, LiveClawConfig, ProviderProfileConfig, ProvidersConfig, ResilienceConfig,
    RuntimeKind, SecurityConfig as AppSecurityConfig, VoiceConfig,
};
use liveclaw_app::runner::build_runner;
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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum DockerWorkerCommand {
    SendAudio { id: u64, audio_base64: String },
    SendText { id: u64, text: String },
    CommitAudio { id: u64 },
    CreateResponse { id: u64 },
    InterruptResponse { id: u64 },
    Close { id: u64 },
}

impl DockerWorkerCommand {
    fn id(&self) -> u64 {
        match self {
            Self::SendAudio { id, .. }
            | Self::SendText { id, .. }
            | Self::CommitAudio { id }
            | Self::CreateResponse { id }
            | Self::InterruptResponse { id }
            | Self::Close { id } => *id,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum DockerWorkerMessage {
    Started,
    Ack {
        id: u64,
        ok: bool,
        error: Option<String>,
    },
    AudioOutput {
        audio_base64: String,
        item_id: String,
    },
    TextOutput {
        text: String,
        item_id: String,
    },
    TranscriptOutput {
        transcript: String,
        item_id: String,
    },
    RuntimeError {
        message: String,
    },
}

struct DockerWorkerEventForwarder {
    tx: mpsc::UnboundedSender<DockerWorkerMessage>,
}

#[async_trait]
impl EventHandler for DockerWorkerEventForwarder {
    async fn on_audio(&self, audio: &[u8], item_id: &str) -> adk_realtime::Result<()> {
        let encoded = base64_encode(audio);
        self.tx
            .send(DockerWorkerMessage::AudioOutput {
                audio_base64: encoded,
                item_id: item_id.to_string(),
            })
            .map_err(|e| {
                adk_realtime::error::RealtimeError::connection(format!(
                    "Failed to send audio output from docker worker: {}",
                    e
                ))
            })
    }

    async fn on_text(&self, text: &str, item_id: &str) -> adk_realtime::Result<()> {
        self.tx
            .send(DockerWorkerMessage::TextOutput {
                text: text.to_string(),
                item_id: item_id.to_string(),
            })
            .map_err(|e| {
                adk_realtime::error::RealtimeError::connection(format!(
                    "Failed to send text output from docker worker: {}",
                    e
                ))
            })
    }

    async fn on_transcript(&self, transcript: &str, item_id: &str) -> adk_realtime::Result<()> {
        self.tx
            .send(DockerWorkerMessage::TranscriptOutput {
                transcript: transcript.to_string(),
                item_id: item_id.to_string(),
            })
            .map_err(|e| {
                adk_realtime::error::RealtimeError::connection(format!(
                    "Failed to send transcript output from docker worker: {}",
                    e
                ))
            })
    }
}

#[derive(Debug, Clone)]
struct DockerRuntimeConfig {
    session_id: String,
    image: String,
    api_key: String,
    model: String,
    voice: String,
    instructions: Option<String>,
    base_url: Option<String>,
}

#[derive(Clone)]
struct DockerRuntimeConnection {
    child: Arc<AsyncMutex<Child>>,
    stdout: Arc<AsyncMutex<Option<tokio::process::ChildStdout>>>,
    command_tx: mpsc::UnboundedSender<DockerWorkerCommand>,
    pending_acks: Arc<AsyncMutex<HashMap<u64, oneshot::Sender<Result<()>>>>>,
}

struct DockerRuntimeBridgeRuntime {
    config: DockerRuntimeConfig,
    event_handler: Arc<dyn EventHandler>,
    connection: AsyncMutex<Option<DockerRuntimeConnection>>,
    next_command_id: AtomicU64,
}

impl DockerRuntimeBridgeRuntime {
    fn new(config: DockerRuntimeConfig, event_handler: Arc<dyn EventHandler>) -> Self {
        Self {
            config,
            event_handler,
            connection: AsyncMutex::new(None),
            next_command_id: AtomicU64::new(1),
        }
    }

    fn docker_runtime_command_args(config: &DockerRuntimeConfig) -> Vec<String> {
        let session_slug = config
            .session_id
            .chars()
            .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
            .take(30)
            .collect::<String>();
        let container_name = format!("liveclaw-runtime-{}", session_slug);

        let mut args = vec![
            "run".to_string(),
            "--rm".to_string(),
            "-i".to_string(),
            "--network".to_string(),
            "host".to_string(),
            "--name".to_string(),
            container_name,
            "-e".to_string(),
            format!("LIVECLAW_RUNTIME_API_KEY={}", config.api_key),
            "-e".to_string(),
            format!("LIVECLAW_RUNTIME_MODEL={}", config.model),
            "-e".to_string(),
            format!("LIVECLAW_RUNTIME_VOICE={}", config.voice),
        ];

        if let Some(instructions) = &config.instructions {
            args.push("-e".to_string());
            args.push(format!("LIVECLAW_RUNTIME_INSTRUCTIONS={}", instructions));
        }
        if let Some(base_url) = &config.base_url {
            args.push("-e".to_string());
            args.push(format!("LIVECLAW_RUNTIME_BASE_URL={}", base_url));
        }

        args.push(config.image.clone());
        args.push("--runtime-worker".to_string());
        args
    }

    fn build_docker_runtime_command(config: &DockerRuntimeConfig) -> Command {
        let mut command = Command::new("docker");
        command
            .args(Self::docker_runtime_command_args(config))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        command
    }

    async fn send_worker_command(&self, command: DockerWorkerCommand) -> Result<()> {
        let connection = {
            let guard = self.connection.lock().await;
            guard
                .as_ref()
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("Docker runtime session is not connected"))?
        };

        let id = command.id();
        let (ack_tx, ack_rx) = oneshot::channel();
        connection.pending_acks.lock().await.insert(id, ack_tx);
        if connection.command_tx.send(command).is_err() {
            connection.pending_acks.lock().await.remove(&id);
            bail!("Failed to send command to docker runtime worker");
        }

        match tokio::time::timeout(Duration::from_secs(15), ack_rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => bail!("Docker runtime worker closed ack channel"),
            Err(_) => {
                connection.pending_acks.lock().await.remove(&id);
                bail!("Timed out waiting for docker runtime worker ack")
            }
        }
    }

    async fn fail_pending_acks(&self, error: &str) {
        if let Some(connection) = self.connection.lock().await.as_ref().cloned() {
            let mut pending = connection.pending_acks.lock().await;
            for (_id, tx) in pending.drain() {
                let _ = tx.send(Err(anyhow::anyhow!(error.to_string())));
            }
        }
    }
}

#[async_trait]
impl SessionRuntime for DockerRuntimeBridgeRuntime {
    async fn connect(&self) -> Result<()> {
        if self.connection.lock().await.is_some() {
            return Ok(());
        }

        let mut command = Self::build_docker_runtime_command(&self.config);
        let mut child = command.spawn().with_context(|| {
            format!(
                "Failed to spawn docker runtime container '{}'",
                self.config.image
            )
        })?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow::anyhow!("Docker runtime child stdout was not piped"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| anyhow::anyhow!("Docker runtime child stderr was not piped"))?;
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow::anyhow!("Docker runtime child stdin was not piped"))?;

        let pending_acks: Arc<AsyncMutex<HashMap<u64, oneshot::Sender<Result<()>>>>> =
            Arc::new(AsyncMutex::new(HashMap::new()));
        let (command_tx, mut command_rx) = mpsc::unbounded_channel::<DockerWorkerCommand>();

        tokio::spawn(async move {
            while let Some(command) = command_rx.recv().await {
                let line = match serde_json::to_string(&command) {
                    Ok(line) => line,
                    Err(e) => {
                        warn!(error = %e, "Failed to serialize docker worker command");
                        break;
                    }
                };

                if let Err(e) = stdin.write_all(line.as_bytes()).await {
                    warn!(error = %e, "Failed writing docker worker command");
                    break;
                }
                if let Err(e) = stdin.write_all(b"\n").await {
                    warn!(error = %e, "Failed writing docker worker newline");
                    break;
                }
                if let Err(e) = stdin.flush().await {
                    warn!(error = %e, "Failed flushing docker worker stdin");
                    break;
                }
            }
        });

        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                warn!(message = %line, "Docker runtime stderr");
            }
        });

        let connection = DockerRuntimeConnection {
            child: Arc::new(AsyncMutex::new(child)),
            stdout: Arc::new(AsyncMutex::new(Some(stdout))),
            command_tx,
            pending_acks,
        };

        *self.connection.lock().await = Some(connection);
        Ok(())
    }

    async fn run(&self) -> Result<()> {
        let connection = {
            let guard = self.connection.lock().await;
            guard
                .as_ref()
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("Docker runtime session is not connected"))?
        };

        let stdout = {
            let mut stdout_guard = connection.stdout.lock().await;
            stdout_guard
                .take()
                .ok_or_else(|| anyhow::anyhow!("Docker runtime stdout stream is unavailable"))?
        };

        let process_result: Result<()> = async {
            let mut lines = BufReader::new(stdout).lines();
            while let Some(line) = lines
                .next_line()
                .await
                .context("Failed reading docker runtime worker output")?
            {
                let message: DockerWorkerMessage = serde_json::from_str(&line).with_context(|| {
                    format!("Failed to parse docker runtime worker message: {}", line)
                })?;

                match message {
                    DockerWorkerMessage::Started => {
                        info!(session_id = %self.config.session_id, "Docker runtime worker started");
                    }
                    DockerWorkerMessage::Ack { id, ok, error } => {
                        if let Some(tx) = connection.pending_acks.lock().await.remove(&id) {
                            if ok {
                                let _ = tx.send(Ok(()));
                            } else {
                                let _ = tx.send(Err(anyhow::anyhow!(
                                    error.unwrap_or_else(|| "Docker runtime worker command failed".to_string())
                                )));
                            }
                        }
                    }
                    DockerWorkerMessage::AudioOutput { audio_base64, item_id } => {
                        let audio = base64::engine::general_purpose::STANDARD
                            .decode(audio_base64.as_bytes())
                            .context("Failed to decode docker worker audio payload")?;
                        self.event_handler.on_audio(&audio, &item_id).await.map_err(|e| {
                            anyhow::anyhow!("Failed to forward docker audio output: {}", e)
                        })?;
                    }
                    DockerWorkerMessage::TextOutput { text, item_id } => {
                        self.event_handler
                            .on_text(&text, &item_id)
                            .await
                            .map_err(|e| anyhow::anyhow!("Failed to forward docker text output: {}", e))?;
                    }
                    DockerWorkerMessage::TranscriptOutput { transcript, item_id } => {
                        self.event_handler
                            .on_transcript(&transcript, &item_id)
                            .await
                            .map_err(|e| anyhow::anyhow!("Failed to forward docker transcript output: {}", e))?;
                    }
                    DockerWorkerMessage::RuntimeError { message } => {
                        bail!("Docker runtime worker error: {}", message);
                    }
                }
            }

            let status = connection
                .child
                .lock()
                .await
                .wait()
                .await
                .context("Failed waiting on docker runtime child process")?;
            bail!("Docker runtime worker exited with status {}", status)
        }
        .await;

        *self.connection.lock().await = None;
        let error_message = process_result
            .as_ref()
            .err()
            .map(|e| e.to_string())
            .unwrap_or_else(|| "Docker runtime worker loop ended".to_string());
        self.fail_pending_acks(&error_message).await;
        process_result
    }

    async fn close(&self) -> Result<()> {
        let connection = { self.connection.lock().await.take() };
        let Some(connection) = connection else {
            return Ok(());
        };

        let close_id = self.next_command_id.fetch_add(1, Ordering::SeqCst);
        let _ = connection
            .command_tx
            .send(DockerWorkerCommand::Close { id: close_id });

        tokio::time::sleep(Duration::from_millis(50)).await;
        if let Err(e) = connection.child.lock().await.kill().await {
            warn!(
                session_id = %self.config.session_id,
                error = %e,
                "Failed to kill docker runtime child"
            );
        }

        let mut pending = connection.pending_acks.lock().await;
        for (_id, tx) in pending.drain() {
            let _ = tx.send(Err(anyhow::anyhow!(
                "Docker runtime worker closed before acknowledging command"
            )));
        }
        Ok(())
    }

    async fn send_audio_base64(&self, audio_base64: &str) -> Result<()> {
        let id = self.next_command_id.fetch_add(1, Ordering::SeqCst);
        self.send_worker_command(DockerWorkerCommand::SendAudio {
            id,
            audio_base64: audio_base64.to_string(),
        })
        .await
    }

    async fn send_text(&self, text: &str) -> Result<()> {
        let id = self.next_command_id.fetch_add(1, Ordering::SeqCst);
        self.send_worker_command(DockerWorkerCommand::SendText {
            id,
            text: text.to_string(),
        })
        .await
    }

    async fn commit_audio(&self) -> Result<()> {
        let id = self.next_command_id.fetch_add(1, Ordering::SeqCst);
        self.send_worker_command(DockerWorkerCommand::CommitAudio { id })
            .await
    }

    async fn create_response(&self) -> Result<()> {
        let id = self.next_command_id.fetch_add(1, Ordering::SeqCst);
        self.send_worker_command(DockerWorkerCommand::CreateResponse { id })
            .await
    }

    async fn interrupt_response(&self) -> Result<()> {
        let id = self.next_command_id.fetch_add(1, Ordering::SeqCst);
        self.send_worker_command(DockerWorkerCommand::InterruptResponse { id })
            .await
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

struct LifecycleNoopAgent;

#[async_trait]
impl Agent for LifecycleNoopAgent {
    fn name(&self) -> &str {
        "liveclaw-lifecycle-ingest"
    }

    fn description(&self) -> &str {
        "No-op agent used to persist transcript events through adk-runner lifecycle."
    }

    fn sub_agents(&self) -> &[Arc<dyn Agent>] {
        &[]
    }

    async fn run(
        &self,
        _ctx: Arc<dyn adk_core::InvocationContext>,
    ) -> adk_core::Result<adk_core::EventStream> {
        Ok(Box::pin(futures::stream::empty::<adk_core::Result<Event>>()))
    }
}

struct TranscriptCompactionSummarizer;

#[async_trait]
impl adk_core::BaseEventsSummarizer for TranscriptCompactionSummarizer {
    async fn summarize_events(&self, events: &[Event]) -> adk_core::Result<Option<Event>> {
        if events.is_empty() {
            return Ok(None);
        }

        let mut summary_chunks = Vec::new();
        for event in events {
            let Some(content) = &event.llm_response.content else {
                continue;
            };
            if let Some(text) = content
                .parts
                .iter()
                .find_map(|part| part.text().map(str::trim))
                .filter(|text| !text.is_empty())
            {
                summary_chunks.push(text.to_string());
            }
        }

        let mut summary_text = if summary_chunks.is_empty() {
            format!("compaction_summary: compacted {} events", events.len())
        } else {
            format!("compaction_summary: {}", summary_chunks.join(" "))
        };
        if summary_text.len() > 4_096 {
            summary_text.truncate(4_096);
        }

        let compacted_content = Content::new("system").with_text(summary_text);
        let mut compaction_event = Event::new("liveclaw-compaction");
        compaction_event.author = "system".to_string();
        compaction_event.llm_response.content = Some(compacted_content.clone());
        compaction_event.actions = EventActions {
            compaction: Some(EventCompaction {
                start_timestamp: events.first().expect("events is non-empty").timestamp,
                end_timestamp: events.last().expect("events is non-empty").timestamp,
                compacted_content,
            }),
            ..Default::default()
        };
        Ok(Some(compaction_event))
    }
}

struct SessionLifecycleManager {
    app_name: String,
    runner: Arc<adk_runner::Runner>,
    session_service: Arc<dyn SessionService>,
    compaction_counts: Arc<RwLock<HashMap<String, usize>>>,
}

impl SessionLifecycleManager {
    fn new(app_name: &str, policy: &MemoryCompactionPolicy) -> Result<Self> {
        let session_service: Arc<dyn SessionService> = Arc::new(InMemorySessionService::new());
        let compaction_config = if policy.enabled {
            Some(EventsCompactionConfig {
                compaction_interval: policy.max_events_threshold.max(1) as u32,
                overlap_size: 0,
                summarizer: Arc::new(TranscriptCompactionSummarizer),
            })
        } else {
            None
        };

        let runner = Arc::new(build_runner(
            Arc::new(LifecycleNoopAgent),
            session_service.clone(),
            None,
            None,
            None,
            compaction_config,
        )?);

        Ok(Self {
            app_name: app_name.to_string(),
            runner,
            session_service,
            compaction_counts: Arc::new(RwLock::new(HashMap::new())),
        })
    }

    async fn initialize_session(&self, user_id: &str, session_id: &str) -> Result<()> {
        let get_result = self
            .session_service
            .get(GetRequest {
                app_name: self.app_name.clone(),
                user_id: user_id.to_string(),
                session_id: session_id.to_string(),
                num_recent_events: None,
                after: None,
            })
            .await;
        if get_result.is_err() {
            self.session_service
                .create(CreateRequest {
                    app_name: self.app_name.clone(),
                    user_id: user_id.to_string(),
                    session_id: Some(session_id.to_string()),
                    state: HashMap::new(),
                })
                .await?;
        }
        self.compaction_counts
            .write()
            .await
            .entry(session_id.to_string())
            .or_insert(0);
        Ok(())
    }

    async fn record_transcript(
        &self,
        user_id: &str,
        session_id: &str,
        transcript: &str,
    ) -> Result<u64> {
        let trimmed = transcript.trim();
        if trimmed.is_empty() {
            return Ok(0);
        }

        self.initialize_session(user_id, session_id).await?;
        let mut stream = self
            .runner
            .run(
                user_id.to_string(),
                session_id.to_string(),
                Content::new("user").with_text(trimmed.to_string()),
            )
            .await
            .map_err(|e| {
                anyhow::anyhow!(
                    "Failed to run lifecycle ingest session '{}': {}",
                    session_id,
                    e
                )
            })?;
        while let Some(event) = stream.next().await {
            event.map_err(|e| {
                anyhow::anyhow!(
                    "Lifecycle ingest stream failed for session '{}': {}",
                    session_id,
                    e
                )
            })?;
        }

        let current = self.current_compaction_count(user_id, session_id).await?;
        let mut guard = self.compaction_counts.write().await;
        let previous = guard.insert(session_id.to_string(), current).unwrap_or(0);
        Ok(current.saturating_sub(previous) as u64)
    }

    async fn snapshot_memory_entries(
        &self,
        user_id: &str,
        session_id: &str,
    ) -> Result<Vec<adk_memory::MemoryEntry>> {
        let session = self
            .session_service
            .get(GetRequest {
                app_name: self.app_name.clone(),
                user_id: user_id.to_string(),
                session_id: session_id.to_string(),
                num_recent_events: None,
                after: None,
            })
            .await?;

        Ok(Self::memory_entries_from_session_events(
            session.events().all(),
        ))
    }

    async fn clear_tracking(&self, session_id: &str) {
        self.compaction_counts.write().await.remove(session_id);
    }

    async fn current_compaction_count(&self, user_id: &str, session_id: &str) -> Result<usize> {
        let session = self
            .session_service
            .get(GetRequest {
                app_name: self.app_name.clone(),
                user_id: user_id.to_string(),
                session_id: session_id.to_string(),
                num_recent_events: None,
                after: None,
            })
            .await?;

        Ok(session
            .events()
            .all()
            .iter()
            .filter(|event| event.actions.compaction.is_some())
            .count())
    }

    fn memory_entries_from_session_events(events: Vec<Event>) -> Vec<adk_memory::MemoryEntry> {
        let latest_compaction = events
            .iter()
            .rev()
            .find_map(|event| event.actions.compaction.clone());
        let boundary = latest_compaction
            .as_ref()
            .map(|compaction| compaction.end_timestamp);

        let mut entries = Vec::new();
        if let Some(compaction) = latest_compaction {
            entries.push(adk_memory::MemoryEntry {
                content: compaction.compacted_content,
                author: "system".to_string(),
                timestamp: compaction.end_timestamp,
            });
        }

        for event in events {
            if event.actions.compaction.is_some() {
                continue;
            }
            if let Some(boundary_ts) = boundary {
                if event.timestamp <= boundary_ts {
                    continue;
                }
            }
            let Some(content) = event.llm_response.content else {
                continue;
            };
            let has_text = content.parts.iter().any(|part| {
                part.text()
                    .map(str::trim)
                    .is_some_and(|text| !text.is_empty())
            });
            if !has_text {
                continue;
            }

            let author = if event.author.trim().is_empty() {
                "assistant".to_string()
            } else {
                event.author
            };
            entries.push(adk_memory::MemoryEntry {
                content,
                author,
                timestamp: event.timestamp,
            });
        }

        entries
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

async fn execute_session_tool_call_direct(
    session_id: &str,
    user_id: &str,
    tool_name: &str,
    arguments: Value,
    tools: HashMap<String, Arc<dyn Tool>>,
) -> Result<(Value, GraphExecutionReport)> {
    let thread_id = format!(
        "{}-direct-{}",
        session_id,
        chrono::Utc::now().timestamp_millis()
    );

    let Some(tool) = tools.get(tool_name) else {
        let mut available_tools = tools.keys().cloned().collect::<Vec<_>>();
        available_tools.sort();
        let result = json!({
            "tool": tool_name,
            "status": "error",
            "message": "Tool is not registered for this session",
            "available_tools": available_tools,
        });
        return Ok((
            result.clone(),
            GraphExecutionReport {
                thread_id,
                completed: true,
                interrupted: false,
                events: vec![GraphExecutionEvent {
                    step: 0,
                    node: "direct_tool".to_string(),
                    action: "node_end".to_string(),
                    detail: "tool not registered for session".to_string(),
                }],
                final_state: json!({
                    "execution_mode": "direct",
                    "tool_result": result,
                }),
            },
        ));
    };

    let tool_ctx = Arc::new(ToolInvocationContext::new(
        format!("direct-tool-{}", chrono::Utc::now().timestamp_millis()),
        user_id.to_string(),
        session_id.to_string(),
    )) as Arc<dyn ToolContext>;

    let result = match tool.execute(tool_ctx, arguments).await {
        Ok(output) => json!({
            "tool": tool_name,
            "status": "ok",
            "result": output,
        }),
        Err(err) => json!({
            "tool": tool_name,
            "status": "error",
            "message": err.to_string(),
        }),
    };

    Ok((
        result.clone(),
        GraphExecutionReport {
            thread_id,
            completed: true,
            interrupted: false,
            events: vec![GraphExecutionEvent {
                step: 0,
                node: "direct_tool".to_string(),
                action: "node_end".to_string(),
                detail: "direct tool execution completed".to_string(),
            }],
            final_state: json!({
                "execution_mode": "direct",
                "tool_result": result,
            }),
        },
    ))
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
    session_lifecycle: Option<Arc<SessionLifecycleManager>>,
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

        if let Some(lifecycle) = &self.session_lifecycle {
            match lifecycle
                .record_transcript(&self.user_id, &self.session_id, transcript)
                .await
            {
                Ok(new_compactions) if new_compactions > 0 => {
                    self.runtime_diagnostics
                        .compactions_applied_total
                        .fetch_add(new_compactions, Ordering::SeqCst);
                    info!(
                        session_id = %self.session_id,
                        compactions = new_compactions,
                        "Applied transcript compaction via adk-runner lifecycle path"
                    );
                }
                Ok(_) => {}
                Err(e) => {
                    warn!(
                        session_id = %self.session_id,
                        user_id = %self.user_id,
                        error = %e,
                        "Failed to record transcript in adk-runner lifecycle path"
                    );
                }
            }

            match lifecycle
                .snapshot_memory_entries(&self.user_id, &self.session_id)
                .await
            {
                Ok(session_entries) => {
                    self.session_memory_entries
                        .write()
                        .await
                        .insert(self.session_id.clone(), session_entries.clone());
                    if let Err(e) = self
                        .memory_store
                        .add_session("liveclaw", &self.user_id, &self.session_id, session_entries)
                        .await
                    {
                        warn!(
                            session_id = %self.session_id,
                            user_id = %self.user_id,
                            error = %e,
                            "Failed to persist transcript memory from adk-runner lifecycle snapshot"
                        );
                    }
                    return;
                }
                Err(e) => {
                    warn!(
                        session_id = %self.session_id,
                        user_id = %self.user_id,
                        error = %e,
                        "Failed to build lifecycle memory snapshot; falling back to local transcript memory path"
                    );
                }
            }
        }

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

    async fn on_text(&self, text: &str, _item_id: &str) -> adk_realtime::Result<()> {
        if self
            .transcript_tx
            .send(SessionTranscriptOutput {
                session_id: self.session_id.clone(),
                text: text.to_string(),
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
        self.persist_transcript_memory(text).await;
        self.persist_transcript_artifact(text).await;
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
    runtime_docker_image: String,
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
    session_lifecycle: Option<Arc<SessionLifecycleManager>>,
    graph_default_enabled: bool,
    graph_recursion_limit: usize,
    sessions: Arc<RwLock<HashMap<String, RealtimeSession>>>,
    audio_tx: mpsc::Sender<SessionAudioOutput>,
    transcript_tx: mpsc::Sender<SessionTranscriptOutput>,
}

struct RunnerAdapterInit {
    runtime_kind: RuntimeKind,
    runtime_docker_image: String,
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
        let session_lifecycle = match SessionLifecycleManager::new(
            "liveclaw",
            &init.memory_compaction_policy,
        ) {
            Ok(manager) => Some(Arc::new(manager)),
            Err(e) => {
                warn!(
                    error = %e,
                    "Failed to initialize adk-runner lifecycle manager; falling back to local transcript lifecycle path"
                );
                None
            }
        };

        Self {
            runtime_kind: init.runtime_kind,
            runtime_docker_image: init.runtime_docker_image,
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
            session_lifecycle,
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

        if let Some(lifecycle) = &self.session_lifecycle {
            lifecycle
                .initialize_session(user_id, session_id)
                .await
                .with_context(|| {
                    format!(
                        "Failed to initialize adk-runner lifecycle session '{}'",
                        session_id
                    )
                })?;
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

        let mut tool_names = protected_tools
            .iter()
            .map(|tool| tool.name().to_string())
            .collect::<Vec<_>>();
        tool_names.sort();
        let merged_instruction = if matches!(self.runtime_kind, RuntimeKind::Native)
            && !tool_names.is_empty()
        {
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

        let event_forwarder = GatewayEventForwarder {
            session_id: session_id.to_string(),
            user_id: user_id.to_string(),
            memory_store: self.memory_store.clone(),
            session_memory_entries: self.session_memory_entries.clone(),
            session_lifecycle: self.session_lifecycle.clone(),
            artifact_service: self.artifact_service.clone(),
            memory_compaction: self.memory_compaction_policy.clone(),
            runtime_diagnostics: self.runtime_diagnostics.clone(),
            audio_sequence: AtomicU64::new(0),
            transcript_sequence: AtomicU64::new(0),
            audio_tx: self.audio_tx.clone(),
            transcript_tx: self.transcript_tx.clone(),
        };

        let mut session_tools = HashMap::new();
        for tool in &protected_tools {
            session_tools.insert(tool.name().to_string(), tool.clone());
        }

        let runtime: Arc<dyn SessionRuntime> = match &self.runtime_kind {
            RuntimeKind::Native => {
                let mut model =
                    OpenAIRealtimeModel::new(self.provider.api_key.clone(), model_id.clone());
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

                let mut realtime_config = RealtimeConfig::default()
                    .with_model(model_id.clone())
                    .with_text_and_audio()
                    .with_server_vad()
                    .with_voice(voice.clone());
                if let Some(instr) = merged_instruction.clone() {
                    realtime_config = realtime_config.with_instruction(instr);
                }

                let mut runner_builder = RealtimeRunner::builder()
                    .model(Arc::new(model))
                    .config(realtime_config)
                    .event_handler(event_forwarder);

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
                Arc::new(RealtimeRunnerRuntime::new(runner))
            }
            RuntimeKind::Docker => Arc::new(DockerRuntimeBridgeRuntime::new(
                DockerRuntimeConfig {
                    session_id: session_id.to_string(),
                    image: self.runtime_docker_image.clone(),
                    api_key: self.provider.api_key.clone(),
                    model: model_id.clone(),
                    voice: voice.clone(),
                    instructions: merged_instruction.clone(),
                    base_url: self.provider.base_url.clone(),
                },
                Arc::new(event_forwarder) as Arc<dyn EventHandler>,
            )),
        };
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

        if let Some(lifecycle) = &self.session_lifecycle {
            lifecycle.clear_tracking(session_id).await;
        }

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

        if graph_enabled {
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
        } else {
            execute_session_tool_call_direct(session_id, &user_id, tool_name, arguments, tools)
                .await
        }
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

const DEFAULT_CONFIG_PATH: &str = "liveclaw.toml";

#[derive(Debug, Clone, PartialEq)]
enum ServiceLogStream {
    Stdout,
    Stderr,
    Both,
}

#[derive(Debug, Clone, PartialEq)]
enum ServiceCommand {
    Install {
        config_path: String,
        force: bool,
    },
    Start,
    Stop,
    Restart,
    Status,
    Logs {
        lines: usize,
        follow: bool,
        stream: ServiceLogStream,
    },
    Uninstall,
}

#[derive(Debug, Clone, PartialEq)]
struct OnboardOptions {
    config_path: String,
    force: bool,
    non_interactive: bool,
    install_service: bool,
    start_service: bool,
    api_key: Option<String>,
    model: Option<String>,
    provider_profile: Option<String>,
    base_url: Option<String>,
    gateway_host: Option<String>,
    gateway_port: Option<u16>,
    require_pairing: Option<bool>,
}

impl Default for OnboardOptions {
    fn default() -> Self {
        Self {
            config_path: DEFAULT_CONFIG_PATH.to_string(),
            force: false,
            non_interactive: false,
            install_service: false,
            start_service: false,
            api_key: None,
            model: None,
            provider_profile: None,
            base_url: None,
            gateway_host: None,
            gateway_port: None,
            require_pairing: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
enum AppCommand {
    Run { config_path: String },
    Doctor { config_path: String },
    RuntimeWorker,
    Onboard(OnboardOptions),
    Service(ServiceCommand),
    Help,
}

#[derive(Debug, Default)]
struct DoctorReport {
    notes: Vec<String>,
    warnings: Vec<String>,
}

fn cli_usage() -> &'static str {
    "Usage:
  liveclaw-app [CONFIG_PATH]
  liveclaw-app --doctor [CONFIG_PATH]
  liveclaw-app doctor [CONFIG_PATH]
  liveclaw-app onboard [CONFIG_PATH] [--force] [--non-interactive]
      [--api-key KEY] [--model MODEL]
      [--provider-profile legacy|openai|openai_compatible]
      [--base-url WS_URL]
      [--gateway-host HOST] [--gateway-port PORT]
      [--require-pairing true|false | --no-pairing]
      [--install-service] [--start-service]
  liveclaw-app service install [--config CONFIG_PATH] [--force]
  liveclaw-app service start|stop|restart|status|uninstall
  liveclaw-app service logs [--lines N] [--follow] [--stream both|stdout|stderr]
  liveclaw-app service doctor [--config CONFIG_PATH]
  liveclaw-app --runtime-worker
"
}

fn parse_bool_flag(raw: &str, flag_name: &str) -> Result<bool> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" | "y" | "on" => Ok(true),
        "false" | "0" | "no" | "n" | "off" => Ok(false),
        _ => bail!(
            "Invalid value '{}' for {} (expected true/false)",
            raw,
            flag_name
        ),
    }
}

fn parse_onboard_args(args: &[String]) -> Result<AppCommand> {
    let mut opts = OnboardOptions::default();
    let mut idx = 0;
    let mut config_set = false;

    while idx < args.len() {
        let arg = &args[idx];
        match arg.as_str() {
            "--force" => {
                opts.force = true;
                idx += 1;
            }
            "--non-interactive" => {
                opts.non_interactive = true;
                idx += 1;
            }
            "--install-service" => {
                opts.install_service = true;
                idx += 1;
            }
            "--start-service" => {
                opts.start_service = true;
                opts.install_service = true;
                idx += 1;
            }
            "--no-pairing" => {
                opts.require_pairing = Some(false);
                idx += 1;
            }
            "--config" => {
                let value = args
                    .get(idx + 1)
                    .ok_or_else(|| anyhow::anyhow!("--config requires a path"))?;
                opts.config_path = value.clone();
                config_set = true;
                idx += 2;
            }
            "--api-key" => {
                let value = args
                    .get(idx + 1)
                    .ok_or_else(|| anyhow::anyhow!("--api-key requires a value"))?;
                opts.api_key = Some(value.clone());
                idx += 2;
            }
            "--model" => {
                let value = args
                    .get(idx + 1)
                    .ok_or_else(|| anyhow::anyhow!("--model requires a value"))?;
                opts.model = Some(value.clone());
                idx += 2;
            }
            "--provider-profile" => {
                let value = args
                    .get(idx + 1)
                    .ok_or_else(|| anyhow::anyhow!("--provider-profile requires a value"))?;
                opts.provider_profile = Some(value.clone());
                idx += 2;
            }
            "--base-url" => {
                let value = args
                    .get(idx + 1)
                    .ok_or_else(|| anyhow::anyhow!("--base-url requires a value"))?;
                opts.base_url = Some(value.clone());
                idx += 2;
            }
            "--gateway-host" => {
                let value = args
                    .get(idx + 1)
                    .ok_or_else(|| anyhow::anyhow!("--gateway-host requires a value"))?;
                opts.gateway_host = Some(value.clone());
                idx += 2;
            }
            "--gateway-port" => {
                let value = args
                    .get(idx + 1)
                    .ok_or_else(|| anyhow::anyhow!("--gateway-port requires a value"))?;
                let port = value
                    .parse::<u16>()
                    .map_err(|e| anyhow::anyhow!("Invalid --gateway-port '{}': {}", value, e))?;
                opts.gateway_port = Some(port);
                idx += 2;
            }
            "--require-pairing" => {
                let value = args
                    .get(idx + 1)
                    .ok_or_else(|| anyhow::anyhow!("--require-pairing requires true/false"))?;
                opts.require_pairing = Some(parse_bool_flag(value, "--require-pairing")?);
                idx += 2;
            }
            _ if arg.starts_with('-') => {
                bail!("Unknown onboard option '{}'\n{}", arg, cli_usage());
            }
            _ => {
                if config_set {
                    bail!(
                        "Unexpected positional argument '{}' for onboard\n{}",
                        arg,
                        cli_usage()
                    );
                }
                opts.config_path = arg.clone();
                config_set = true;
                idx += 1;
            }
        }
    }

    Ok(AppCommand::Onboard(opts))
}

fn parse_service_log_stream(raw: &str) -> Result<ServiceLogStream> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "stdout" | "out" => Ok(ServiceLogStream::Stdout),
        "stderr" | "err" => Ok(ServiceLogStream::Stderr),
        "both" | "all" => Ok(ServiceLogStream::Both),
        _ => bail!(
            "Invalid --stream value '{}' (expected both|stdout|stderr)",
            raw
        ),
    }
}

fn parse_service_args(args: &[String]) -> Result<AppCommand> {
    let action = args
        .first()
        .ok_or_else(|| anyhow::anyhow!("service requires a subcommand\n{}", cli_usage()))?;

    match action.as_str() {
        "install" => {
            let mut config_path = DEFAULT_CONFIG_PATH.to_string();
            let mut force = false;
            let mut idx = 1;
            while idx < args.len() {
                match args[idx].as_str() {
                    "--config" => {
                        let value = args.get(idx + 1).ok_or_else(|| {
                            anyhow::anyhow!("service install --config requires a path")
                        })?;
                        config_path = value.clone();
                        idx += 2;
                    }
                    "--force" => {
                        force = true;
                        idx += 1;
                    }
                    other => {
                        bail!(
                            "Unknown service install option '{}'\n{}",
                            other,
                            cli_usage()
                        );
                    }
                }
            }
            Ok(AppCommand::Service(ServiceCommand::Install {
                config_path,
                force,
            }))
        }
        "start" => Ok(AppCommand::Service(ServiceCommand::Start)),
        "stop" => Ok(AppCommand::Service(ServiceCommand::Stop)),
        "restart" => Ok(AppCommand::Service(ServiceCommand::Restart)),
        "status" => Ok(AppCommand::Service(ServiceCommand::Status)),
        "logs" => {
            let mut lines: usize = 80;
            let mut follow = false;
            let mut stream = ServiceLogStream::Both;
            let mut idx = 1;
            while idx < args.len() {
                match args[idx].as_str() {
                    "--lines" => {
                        let value = args.get(idx + 1).ok_or_else(|| {
                            anyhow::anyhow!("service logs --lines requires a number")
                        })?;
                        lines = value
                            .parse::<usize>()
                            .map_err(|e| anyhow::anyhow!("Invalid --lines '{}': {}", value, e))?;
                        if lines == 0 {
                            bail!("service logs --lines must be greater than zero");
                        }
                        idx += 2;
                    }
                    "--follow" => {
                        follow = true;
                        idx += 1;
                    }
                    "--stream" => {
                        let value = args.get(idx + 1).ok_or_else(|| {
                            anyhow::anyhow!("service logs --stream requires a value")
                        })?;
                        stream = parse_service_log_stream(value)?;
                        idx += 2;
                    }
                    "--stdout" => {
                        stream = ServiceLogStream::Stdout;
                        idx += 1;
                    }
                    "--stderr" => {
                        stream = ServiceLogStream::Stderr;
                        idx += 1;
                    }
                    other => {
                        bail!("Unknown service logs option '{}'\n{}", other, cli_usage());
                    }
                }
            }
            Ok(AppCommand::Service(ServiceCommand::Logs {
                lines,
                follow,
                stream,
            }))
        }
        "doctor" => {
            let mut config_path = DEFAULT_CONFIG_PATH.to_string();
            let mut idx = 1;
            while idx < args.len() {
                match args[idx].as_str() {
                    "--config" => {
                        let value = args.get(idx + 1).ok_or_else(|| {
                            anyhow::anyhow!("service doctor --config requires a path")
                        })?;
                        config_path = value.clone();
                        idx += 2;
                    }
                    other => {
                        bail!("Unknown service doctor option '{}'\n{}", other, cli_usage());
                    }
                }
            }
            Ok(AppCommand::Doctor { config_path })
        }
        "uninstall" => Ok(AppCommand::Service(ServiceCommand::Uninstall)),
        other => bail!("Unknown service subcommand '{}'\n{}", other, cli_usage()),
    }
}

fn parse_command() -> Result<AppCommand> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        return Ok(AppCommand::Run {
            config_path: DEFAULT_CONFIG_PATH.to_string(),
        });
    }

    let first = &args[0];
    match first.as_str() {
        "--runtime-worker" => Ok(AppCommand::RuntimeWorker),
        "--doctor" | "doctor" => Ok(AppCommand::Doctor {
            config_path: args
                .get(1)
                .cloned()
                .unwrap_or_else(|| DEFAULT_CONFIG_PATH.to_string()),
        }),
        "onboard" | "--onboard" => parse_onboard_args(&args[1..]),
        "service" => parse_service_args(&args[1..]),
        "--help" | "-h" | "help" => Ok(AppCommand::Help),
        _ if first.starts_with('-') => {
            bail!("Unknown option '{}'\n{}", first, cli_usage())
        }
        _ => Ok(AppCommand::Run {
            config_path: first.clone(),
        }),
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

fn home_dir() -> Result<PathBuf> {
    if let Ok(home) = std::env::var("HOME") {
        let trimmed = home.trim();
        if !trimmed.is_empty() {
            return Ok(PathBuf::from(trimmed));
        }
    }
    if let Ok(home) = std::env::var("USERPROFILE") {
        let trimmed = home.trim();
        if !trimmed.is_empty() {
            return Ok(PathBuf::from(trimmed));
        }
    }
    bail!("Unable to resolve home directory from HOME/USERPROFILE")
}

fn normalize_provider_profile(raw: &str) -> Result<String> {
    let normalized = raw.trim().to_ascii_lowercase().replace('-', "_");
    match normalized.as_str() {
        "legacy" | "openai" | "openai_compatible" => Ok(normalized),
        _ => bail!(
            "Unsupported provider profile '{}'. Use legacy, openai, or openai_compatible.",
            raw
        ),
    }
}

fn prompt_text(prompt: &str, default: Option<&str>, required: bool) -> Result<String> {
    loop {
        match default {
            Some(value) => print!("{} [{}]: ", prompt, value),
            None => print!("{}: ", prompt),
        }
        io::stdout()
            .flush()
            .context("Failed to flush prompt to stdout")?;

        let mut line = String::new();
        io::stdin()
            .read_line(&mut line)
            .context("Failed to read onboarding input")?;
        let trimmed = line.trim();

        if trimmed.is_empty() {
            if let Some(value) = default {
                return Ok(value.to_string());
            }
            if required {
                println!("Value is required.");
                continue;
            }
            return Ok(String::new());
        }

        return Ok(trimmed.to_string());
    }
}

fn prompt_bool(prompt: &str, default: bool) -> Result<bool> {
    let default_text = if default { "yes" } else { "no" };
    loop {
        let value = prompt_text(prompt, Some(default_text), false)?;
        match value.trim().to_ascii_lowercase().as_str() {
            "yes" | "y" | "true" | "1" | "on" => return Ok(true),
            "no" | "n" | "false" | "0" | "off" => return Ok(false),
            _ => println!("Please enter yes or no."),
        }
    }
}

fn prompt_u16(prompt: &str, default: u16) -> Result<u16> {
    loop {
        let value = prompt_text(prompt, Some(&default.to_string()), true)?;
        match value.parse::<u16>() {
            Ok(port) => return Ok(port),
            Err(_) => println!("Please enter a valid port number (0-65535)."),
        }
    }
}

fn build_onboard_config(opts: &OnboardOptions) -> Result<LiveClawConfig> {
    let mut config = LiveClawConfig::default();

    let provider_profile = if let Some(profile) = &opts.provider_profile {
        normalize_provider_profile(profile)?
    } else if opts.non_interactive {
        "legacy".to_string()
    } else {
        normalize_provider_profile(&prompt_text(
            "Provider profile (legacy/openai/openai_compatible)",
            Some("legacy"),
            true,
        )?)?
    };

    let api_key = if let Some(key) = &opts.api_key {
        let trimmed = key.trim();
        if trimmed.is_empty() {
            bail!("--api-key cannot be empty");
        }
        trimmed.to_string()
    } else {
        let env_key = effective_api_key("");
        if !env_key.trim().is_empty() {
            env_key
        } else if opts.non_interactive {
            bail!(
                "Missing API key for non-interactive onboarding. Provide --api-key or set LIVECLAW_API_KEY/OPENAI_API_KEY."
            );
        } else {
            let entered = prompt_text(
                "Realtime API key (input is echoed; prefer env vars in shared terminals)",
                None,
                true,
            )?;
            if entered.trim().is_empty() {
                bail!("API key is required");
            }
            entered
        }
    };

    let default_model = "gpt-4o-realtime-preview-2024-12-17";
    let model = if let Some(model) = &opts.model {
        let trimmed = model.trim();
        if trimmed.is_empty() {
            bail!("--model cannot be empty");
        }
        trimmed.to_string()
    } else if opts.non_interactive {
        default_model.to_string()
    } else {
        prompt_text("Realtime model", Some(default_model), true)?
    };

    let gateway_host = if let Some(host) = &opts.gateway_host {
        let trimmed = host.trim();
        if trimmed.is_empty() {
            bail!("--gateway-host cannot be empty");
        }
        trimmed.to_string()
    } else if opts.non_interactive {
        config.gateway.host.clone()
    } else {
        prompt_text("Gateway host", Some(&config.gateway.host), true)?
    };

    let gateway_port = if let Some(port) = opts.gateway_port {
        port
    } else if opts.non_interactive {
        config.gateway.port
    } else {
        prompt_u16("Gateway port", config.gateway.port)?
    };

    let require_pairing = if let Some(require_pairing) = opts.require_pairing {
        require_pairing
    } else if opts.non_interactive {
        config.gateway.require_pairing
    } else {
        prompt_bool(
            "Require pairing for gateway connections",
            config.gateway.require_pairing,
        )?
    };

    let profile_base_url = if provider_profile == "openai_compatible" {
        if let Some(base_url) = &opts.base_url {
            let trimmed = base_url.trim();
            if trimmed.is_empty() {
                bail!("--base-url cannot be empty for openai_compatible profile");
            }
            validate_ws_base_url(trimmed, "openai_compatible")?;
            Some(trimmed.to_string())
        } else if opts.non_interactive {
            bail!("openai_compatible profile requires --base-url in non-interactive onboarding");
        } else {
            let entered = prompt_text(
                "OpenAI-compatible realtime WebSocket base URL (ws:// or wss://)",
                None,
                true,
            )?;
            validate_ws_base_url(&entered, "openai_compatible")?;
            Some(entered)
        }
    } else {
        opts.base_url
            .as_ref()
            .and_then(|v| non_empty(Some(v.clone())))
    };

    config.gateway.host = gateway_host;
    config.gateway.port = gateway_port;
    config.gateway.require_pairing = require_pairing;
    config.voice.model = model.clone();
    config.voice.api_key = api_key.clone();
    config.providers.active_profile = provider_profile.clone();

    match provider_profile.as_str() {
        "legacy" => {
            config.voice.provider = if profile_base_url.is_some() {
                "openai_compatible".to_string()
            } else {
                "openai".to_string()
            };
            config.voice.base_url = profile_base_url.clone();
        }
        "openai" => {
            config.voice.provider = "openai".to_string();
            config.providers.openai.enabled = true;
            config.providers.openai.api_key = api_key;
            config.providers.openai.model = model;
            config.providers.openai.base_url = profile_base_url.clone();
        }
        "openai_compatible" => {
            config.voice.provider = "openai_compatible".to_string();
            config.providers.openai_compatible.enabled = true;
            config.providers.openai_compatible.api_key = api_key;
            config.providers.openai_compatible.model = model;
            config.providers.openai_compatible.base_url = profile_base_url;
        }
        _ => unreachable!("profile normalized"),
    }

    Ok(config)
}

fn write_onboard_config(path: &str, config: &LiveClawConfig, force: bool) -> Result<()> {
    let target = PathBuf::from(path);
    if target.exists() && !force {
        bail!(
            "Refusing to overwrite existing config '{}'. Re-run with --force to overwrite.",
            path
        );
    }
    if let Some(parent) = target.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).with_context(|| {
                format!(
                    "Failed to create parent directory '{}'",
                    parent.to_string_lossy()
                )
            })?;
        }
    }
    let serialized =
        toml::to_string_pretty(config).context("Failed to serialize onboarding configuration")?;
    fs::write(&target, serialized).with_context(|| {
        format!(
            "Failed to write onboarding configuration to '{}'",
            target.to_string_lossy()
        )
    })?;
    Ok(())
}

fn run_onboarding(opts: OnboardOptions) -> Result<()> {
    println!("LiveClaw onboarding");
    let config = build_onboard_config(&opts)?;
    write_onboard_config(&opts.config_path, &config, opts.force)?;

    let loaded = LiveClawConfig::load(&opts.config_path)?;
    let (provider, report) = validate_runtime_and_provider(&loaded, true)?;

    println!("Wrote config: {}", opts.config_path);
    print_doctor_report(&opts.config_path, &provider, &report);

    if opts.install_service {
        run_service_command(ServiceCommand::Install {
            config_path: opts.config_path.clone(),
            force: opts.force,
        })?;
        if opts.start_service {
            run_service_command(ServiceCommand::Start)?;
        }
    }

    println!(
        "Next step: cargo run -p liveclaw-app -- {}",
        opts.config_path
    );
    Ok(())
}

fn run_command_capture(mut cmd: StdCommand, context: &str) -> Result<String> {
    let output = cmd
        .output()
        .with_context(|| format!("Failed to execute {}", context))?;
    if !output.status.success() {
        bail!(
            "{} failed (exit={}): {}{}",
            context,
            output.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn print_text_file_tail(path: &Path, lines: usize, label: &str) -> Result<()> {
    println!("--- {} ({}) ---", label, path.to_string_lossy());
    if !path.exists() {
        println!("(missing)");
        return Ok(());
    }

    let content = fs::read_to_string(path)
        .with_context(|| format!("Failed reading service log '{}'", path.to_string_lossy()))?;
    let mut tail = content.lines().rev().take(lines).collect::<Vec<_>>();
    tail.reverse();
    if tail.is_empty() {
        println!("(empty)");
    } else {
        for line in tail {
            println!("{}", line);
        }
    }
    Ok(())
}

fn follow_text_files(paths: &[PathBuf], lines: usize, context: &str) -> Result<()> {
    let mut existing = Vec::new();
    for path in paths {
        if path.exists() {
            existing.push(path.clone());
        } else {
            println!("Missing log file: {}", path.to_string_lossy());
        }
    }
    if existing.is_empty() {
        println!("No log files found to follow.");
        return Ok(());
    }

    let status = StdCommand::new("tail")
        .arg("-n")
        .arg(lines.to_string())
        .arg("-f")
        .args(existing.iter().map(|p| p.as_os_str()))
        .status()
        .with_context(|| format!("Failed to execute {}", context))?;
    if !status.success() {
        bail!("{} failed (exit={})", context, status.code().unwrap_or(-1));
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn service_manifest_path() -> Result<PathBuf> {
    let home = home_dir()?;
    Ok(home
        .join("Library")
        .join("LaunchAgents")
        .join("ai.liveclaw.gateway.plist"))
}

#[cfg(target_os = "macos")]
fn service_log_paths() -> Result<(PathBuf, PathBuf)> {
    let logs_dir = home_dir()?.join(".liveclaw").join("logs");
    Ok((
        logs_dir.join("service.out.log"),
        logs_dir.join("service.err.log"),
    ))
}

#[cfg(target_os = "linux")]
fn service_manifest_path() -> Result<PathBuf> {
    let home = home_dir()?;
    Ok(home
        .join(".config")
        .join("systemd")
        .join("user")
        .join("liveclaw.service"))
}

fn service_manifest_missing(path: &Path) -> Result<()> {
    if !path.exists() {
        bail!(
            "LiveClaw service is not installed (missing '{}')",
            path.to_string_lossy()
        );
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn run_service_command(command: ServiceCommand) -> Result<()> {
    let manifest = service_manifest_path()?;
    let label = "ai.liveclaw.gateway";

    match command {
        ServiceCommand::Install { config_path, force } => {
            let config = PathBuf::from(&config_path);
            if !config.exists() {
                bail!(
                    "Config '{}' does not exist. Run onboarding first.",
                    config_path
                );
            }
            if manifest.exists() && !force {
                bail!(
                    "Service manifest '{}' already exists. Use --force to overwrite.",
                    manifest.to_string_lossy()
                );
            }

            let exe = std::env::current_exe().context("Failed to resolve current executable")?;
            let parent = manifest.parent().ok_or_else(|| {
                anyhow::anyhow!(
                    "Invalid service manifest path '{}'",
                    manifest.to_string_lossy()
                )
            })?;
            fs::create_dir_all(parent)?;

            let logs_dir = home_dir()?.join(".liveclaw").join("logs");
            fs::create_dir_all(&logs_dir)?;
            let stdout_path = logs_dir.join("service.out.log");
            let stderr_path = logs_dir.join("service.err.log");

            let plist = format!(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>{label}</string>
  <key>ProgramArguments</key>
  <array>
    <string>{exe}</string>
    <string>{cfg}</string>
  </array>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <true/>
  <key>StandardOutPath</key>
  <string>{stdout}</string>
  <key>StandardErrorPath</key>
  <string>{stderr}</string>
</dict>
</plist>
"#,
                label = label,
                exe = exe.to_string_lossy(),
                cfg = config.to_string_lossy(),
                stdout = stdout_path.to_string_lossy(),
                stderr = stderr_path.to_string_lossy()
            );

            fs::write(&manifest, plist).with_context(|| {
                format!(
                    "Failed writing launchd plist '{}'",
                    manifest.to_string_lossy()
                )
            })?;
            println!("Installed launchd manifest: {}", manifest.to_string_lossy());
        }
        ServiceCommand::Start => {
            service_manifest_missing(&manifest)?;
            run_command_capture(
                {
                    let mut cmd = StdCommand::new("launchctl");
                    cmd.arg("load").arg("-w").arg(&manifest);
                    cmd
                },
                "launchctl load",
            )?;
            println!("LiveClaw service started.");
        }
        ServiceCommand::Stop => {
            service_manifest_missing(&manifest)?;
            run_command_capture(
                {
                    let mut cmd = StdCommand::new("launchctl");
                    cmd.arg("unload").arg("-w").arg(&manifest);
                    cmd
                },
                "launchctl unload",
            )?;
            println!("LiveClaw service stopped.");
        }
        ServiceCommand::Restart => {
            run_service_command(ServiceCommand::Stop)?;
            run_service_command(ServiceCommand::Start)?;
        }
        ServiceCommand::Status => {
            if !manifest.exists() {
                println!("LiveClaw service status: not installed");
                return Ok(());
            }
            let list = run_command_capture(
                {
                    let mut cmd = StdCommand::new("launchctl");
                    cmd.arg("list");
                    cmd
                },
                "launchctl list",
            )?;
            let active = list.lines().any(|line| line.contains(label));
            println!(
                "LiveClaw service status: {}",
                if active { "active" } else { "inactive" }
            );
        }
        ServiceCommand::Logs {
            lines,
            follow,
            stream,
        } => {
            service_manifest_missing(&manifest)?;
            let (stdout_path, stderr_path) = service_log_paths()?;

            if follow {
                match stream {
                    ServiceLogStream::Stdout => {
                        follow_text_files(&[stdout_path], lines, "tail -f (launchd stdout logs)")?
                    }
                    ServiceLogStream::Stderr => {
                        follow_text_files(&[stderr_path], lines, "tail -f (launchd stderr logs)")?
                    }
                    ServiceLogStream::Both => follow_text_files(
                        &[stdout_path, stderr_path],
                        lines,
                        "tail -f (launchd logs)",
                    )?,
                }
            } else {
                match stream {
                    ServiceLogStream::Stdout => {
                        print_text_file_tail(&stdout_path, lines, "launchd stdout")
                    }
                    ServiceLogStream::Stderr => {
                        print_text_file_tail(&stderr_path, lines, "launchd stderr")
                    }
                    ServiceLogStream::Both => {
                        print_text_file_tail(&stdout_path, lines, "launchd stdout")?;
                        print_text_file_tail(&stderr_path, lines, "launchd stderr")
                    }
                }?;
            }
        }
        ServiceCommand::Uninstall => {
            if manifest.exists() {
                let _ = run_service_command(ServiceCommand::Stop);
                fs::remove_file(&manifest).with_context(|| {
                    format!(
                        "Failed to remove launchd plist '{}'",
                        manifest.to_string_lossy()
                    )
                })?;
                println!("Removed launchd manifest: {}", manifest.to_string_lossy());
            } else {
                println!("LiveClaw service not installed.");
            }
        }
    }

    Ok(())
}

#[cfg(target_os = "linux")]
fn systemd_quote(input: &str) -> String {
    let escaped = input.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{}\"", escaped)
}

#[cfg(target_os = "linux")]
fn run_service_command(command: ServiceCommand) -> Result<()> {
    let manifest = service_manifest_path()?;
    let unit_name = "liveclaw.service";

    match command {
        ServiceCommand::Install { config_path, force } => {
            let config = PathBuf::from(&config_path);
            if !config.exists() {
                bail!(
                    "Config '{}' does not exist. Run onboarding first.",
                    config_path
                );
            }
            if manifest.exists() && !force {
                bail!(
                    "Service manifest '{}' already exists. Use --force to overwrite.",
                    manifest.to_string_lossy()
                );
            }
            let exe = std::env::current_exe().context("Failed to resolve current executable")?;
            let parent = manifest.parent().ok_or_else(|| {
                anyhow::anyhow!(
                    "Invalid service manifest path '{}'",
                    manifest.to_string_lossy()
                )
            })?;
            fs::create_dir_all(parent)?;

            let unit = format!(
                "[Unit]\nDescription=LiveClaw gateway runtime\nAfter=network-online.target\n\n[Service]\nType=simple\nExecStart={} {}\nRestart=always\nRestartSec=2\nEnvironment=RUST_LOG=info\n\n[Install]\nWantedBy=default.target\n",
                systemd_quote(&exe.to_string_lossy()),
                systemd_quote(&config.to_string_lossy())
            );
            fs::write(&manifest, unit).with_context(|| {
                format!(
                    "Failed writing systemd unit '{}'",
                    manifest.to_string_lossy()
                )
            })?;

            run_command_capture(
                {
                    let mut cmd = StdCommand::new("systemctl");
                    cmd.arg("--user").arg("daemon-reload");
                    cmd
                },
                "systemctl --user daemon-reload",
            )?;

            println!(
                "Installed systemd user unit: {}",
                manifest.to_string_lossy()
            );
        }
        ServiceCommand::Start => {
            service_manifest_missing(&manifest)?;
            run_command_capture(
                {
                    let mut cmd = StdCommand::new("systemctl");
                    cmd.arg("--user").arg("start").arg(unit_name);
                    cmd
                },
                "systemctl --user start",
            )?;
            println!("LiveClaw service started.");
        }
        ServiceCommand::Stop => {
            service_manifest_missing(&manifest)?;
            run_command_capture(
                {
                    let mut cmd = StdCommand::new("systemctl");
                    cmd.arg("--user").arg("stop").arg(unit_name);
                    cmd
                },
                "systemctl --user stop",
            )?;
            println!("LiveClaw service stopped.");
        }
        ServiceCommand::Restart => {
            service_manifest_missing(&manifest)?;
            run_command_capture(
                {
                    let mut cmd = StdCommand::new("systemctl");
                    cmd.arg("--user").arg("restart").arg(unit_name);
                    cmd
                },
                "systemctl --user restart",
            )?;
            println!("LiveClaw service restarted.");
        }
        ServiceCommand::Status => {
            if !manifest.exists() {
                println!("LiveClaw service status: not installed");
                return Ok(());
            }

            let status = StdCommand::new("systemctl")
                .arg("--user")
                .arg("is-active")
                .arg(unit_name)
                .output()
                .context("Failed to execute systemctl --user is-active")?;
            let active = status.status.success()
                && String::from_utf8_lossy(&status.stdout).trim().eq("active");
            println!(
                "LiveClaw service status: {}",
                if active { "active" } else { "inactive" }
            );
        }
        ServiceCommand::Logs {
            lines,
            follow,
            stream,
        } => {
            service_manifest_missing(&manifest)?;
            if !matches!(stream, ServiceLogStream::Both) {
                println!(
                    "Note: --stream filtering is not supported for journalctl; showing combined logs."
                );
            }

            let mut cmd = StdCommand::new("journalctl");
            cmd.arg("--user")
                .arg("-u")
                .arg(unit_name)
                .arg("-n")
                .arg(lines.to_string())
                .arg("--no-pager");
            if follow {
                cmd.arg("-f");
                let status = cmd
                    .status()
                    .context("Failed to execute journalctl --user (follow)")?;
                if !status.success() {
                    bail!(
                        "journalctl --user follow failed (exit={})",
                        status.code().unwrap_or(-1)
                    );
                }
            } else {
                let output = run_command_capture(cmd, "journalctl --user")?;
                if output.is_empty() {
                    println!("(no logs yet)");
                } else {
                    println!("{}", output);
                }
            }
        }
        ServiceCommand::Uninstall => {
            if manifest.exists() {
                let _ = run_service_command(ServiceCommand::Stop);
                fs::remove_file(&manifest).with_context(|| {
                    format!(
                        "Failed to remove systemd unit '{}'",
                        manifest.to_string_lossy()
                    )
                })?;
                let _ = run_command_capture(
                    {
                        let mut cmd = StdCommand::new("systemctl");
                        cmd.arg("--user").arg("daemon-reload");
                        cmd
                    },
                    "systemctl --user daemon-reload",
                );
                println!("Removed systemd user unit: {}", manifest.to_string_lossy());
            } else {
                println!("LiveClaw service not installed.");
            }
        }
    }

    Ok(())
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn run_service_command(_command: ServiceCommand) -> Result<()> {
    bail!("Service management is currently supported on macOS and Linux only")
}

fn required_env(name: &str) -> Result<String> {
    std::env::var(name).with_context(|| format!("Missing required environment variable {}", name))
}

async fn run_docker_runtime_worker() -> Result<()> {
    let api_key = required_env("LIVECLAW_RUNTIME_API_KEY")?;
    let model_id = required_env("LIVECLAW_RUNTIME_MODEL")?;
    let voice = std::env::var("LIVECLAW_RUNTIME_VOICE").unwrap_or_else(|_| "alloy".to_string());
    let instructions = non_empty(std::env::var("LIVECLAW_RUNTIME_INSTRUCTIONS").ok());
    let base_url = non_empty(std::env::var("LIVECLAW_RUNTIME_BASE_URL").ok());

    let mut model = OpenAIRealtimeModel::new(api_key, model_id.clone());
    if let Some(url) = base_url {
        model = model.with_base_url(url);
    }

    let mut realtime_config = RealtimeConfig::default()
        .with_model(model_id)
        .with_text_and_audio()
        .with_server_vad()
        .with_voice(voice);
    if let Some(instruction_text) = instructions {
        realtime_config = realtime_config.with_instruction(instruction_text);
    }

    let (msg_tx, mut msg_rx) = mpsc::unbounded_channel::<DockerWorkerMessage>();
    let event_handler = DockerWorkerEventForwarder { tx: msg_tx.clone() };

    let runner = Arc::new(
        RealtimeRunner::builder()
            .model(Arc::new(model))
            .config(realtime_config)
            .event_handler(event_handler)
            .build()
            .map_err(|e| anyhow::anyhow!("Failed to build docker runtime worker runner: {}", e))?,
    );
    runner
        .connect()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to connect docker runtime worker runner: {}", e))?;

    let _ = msg_tx.send(DockerWorkerMessage::Started);

    let runtime_runner = runner.clone();
    let runtime_msg_tx = msg_tx.clone();
    tokio::spawn(async move {
        if let Err(e) = runtime_runner.run().await {
            let _ = runtime_msg_tx.send(DockerWorkerMessage::RuntimeError {
                message: format!("Realtime worker loop ended: {}", e),
            });
        }
    });

    tokio::spawn(async move {
        let mut stdout = tokio::io::stdout();
        while let Some(message) = msg_rx.recv().await {
            let line = match serde_json::to_string(&message) {
                Ok(line) => line,
                Err(e) => {
                    warn!(error = %e, "Failed to serialize docker runtime worker message");
                    break;
                }
            };

            if let Err(e) = stdout.write_all(line.as_bytes()).await {
                warn!(error = %e, "Failed writing docker runtime worker stdout");
                break;
            }
            if let Err(e) = stdout.write_all(b"\n").await {
                warn!(error = %e, "Failed writing docker runtime worker newline");
                break;
            }
            if let Err(e) = stdout.flush().await {
                warn!(error = %e, "Failed flushing docker runtime worker stdout");
                break;
            }
        }
    });

    let stdin = tokio::io::stdin();
    let mut lines = BufReader::new(stdin).lines();
    while let Some(line) = lines
        .next_line()
        .await
        .context("Failed reading docker runtime worker stdin")?
    {
        let command: DockerWorkerCommand = match serde_json::from_str(&line) {
            Ok(command) => command,
            Err(e) => {
                let _ = msg_tx.send(DockerWorkerMessage::RuntimeError {
                    message: format!("Invalid docker runtime worker command: {}", e),
                });
                continue;
            }
        };

        let id = command.id();
        let command_result = match command {
            DockerWorkerCommand::SendAudio { audio_base64, .. } => runner
                .send_audio(&audio_base64)
                .await
                .map_err(|e| anyhow::anyhow!("send_audio failed: {}", e)),
            DockerWorkerCommand::SendText { text, .. } => runner
                .send_text(&text)
                .await
                .map_err(|e| anyhow::anyhow!("send_text failed: {}", e)),
            DockerWorkerCommand::CommitAudio { .. } => runner
                .commit_audio()
                .await
                .map_err(|e| anyhow::anyhow!("commit_audio failed: {}", e)),
            DockerWorkerCommand::CreateResponse { .. } => runner
                .create_response()
                .await
                .map_err(|e| anyhow::anyhow!("create_response failed: {}", e)),
            DockerWorkerCommand::InterruptResponse { .. } => runner
                .interrupt()
                .await
                .map_err(|e| anyhow::anyhow!("interrupt_response failed: {}", e)),
            DockerWorkerCommand::Close { .. } => {
                let close_result = runner
                    .close()
                    .await
                    .map_err(|e| anyhow::anyhow!("close failed: {}", e));
                let _ = msg_tx.send(DockerWorkerMessage::Ack {
                    id,
                    ok: close_result.is_ok(),
                    error: close_result.err().map(|err| err.to_string()),
                });
                return Ok(());
            }
        };

        let _ = msg_tx.send(DockerWorkerMessage::Ack {
            id,
            ok: command_result.is_ok(),
            error: command_result.err().map(|err| err.to_string()),
        });
    }

    let _ = runner.close().await;
    Ok(())
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
    let command = parse_command()?;
    match &command {
        AppCommand::RuntimeWorker => return run_docker_runtime_worker().await,
        AppCommand::Help => {
            print!("{}", cli_usage());
            return Ok(());
        }
        AppCommand::Onboard(opts) => return run_onboarding(opts.clone()),
        AppCommand::Service(service_command) => {
            return run_service_command(service_command.clone())
        }
        AppCommand::Run { .. } | AppCommand::Doctor { .. } => {}
    }
    let config_path = match &command {
        AppCommand::Run { config_path } | AppCommand::Doctor { config_path } => config_path,
        _ => unreachable!("command dispatch handled above"),
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
            runtime_docker_image: config.runtime.docker_image.clone(),
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
                runtime_docker_image: "zavoraai/liveclaw-runtime:test".to_string(),
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
            session_lifecycle: None,
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
            session_lifecycle: None,
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
    async fn test_execute_session_tool_call_direct_reads_workspace_file() {
        let workspace = std::env::temp_dir().join(format!(
            "liveclaw-direct-read-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let target_file = workspace.join("notes").join("direct.txt");
        std::fs::create_dir_all(target_file.parent().unwrap()).unwrap();
        std::fs::write(&target_file, "direct tool read success").unwrap();

        let adapter = test_adapter_with_workspace("full", vec![], &workspace);
        let tools = adapter
            .build_protected_tools("principal-1", "full")
            .unwrap()
            .into_iter()
            .map(|tool| (tool.name().to_string(), tool))
            .collect::<HashMap<_, _>>();

        let (result, report) = execute_session_tool_call_direct(
            "sess-direct-read-1",
            "principal-1",
            "read_workspace_file",
            json!({
                "path": "notes/direct.txt",
                "max_bytes": 256,
            }),
            tools,
        )
        .await
        .unwrap();

        assert_eq!(result["status"], json!("ok"));
        assert_eq!(result["tool"], json!("read_workspace_file"));
        assert_eq!(result["result"]["path"], json!("notes/direct.txt"));
        let content = result["result"]["content"].as_str().unwrap_or_default();
        assert!(
            content.contains("direct tool read success"),
            "unexpected read content: {}",
            content
        );
        assert!(report.completed);
        assert!(!report.interrupted);
        assert_eq!(report.final_state["execution_mode"], json!("direct"));

        let _ = std::fs::remove_dir_all(workspace);
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

    #[tokio::test]
    async fn test_session_lifecycle_manager_records_transcript_entries() {
        let manager = SessionLifecycleManager::new(
            "liveclaw",
            &MemoryCompactionPolicy {
                enabled: false,
                max_events_threshold: 8,
            },
        )
        .unwrap();

        manager
            .initialize_session("principal-1", "sess-lifecycle-1")
            .await
            .unwrap();
        let compactions = manager
            .record_transcript("principal-1", "sess-lifecycle-1", "hello lifecycle")
            .await
            .unwrap();
        assert_eq!(compactions, 0);

        let snapshot = manager
            .snapshot_memory_entries("principal-1", "sess-lifecycle-1")
            .await
            .unwrap();
        assert_eq!(snapshot.len(), 1);
        let text = snapshot[0]
            .content
            .parts
            .iter()
            .find_map(Part::text)
            .unwrap_or_default();
        assert!(text.contains("hello lifecycle"));
    }

    #[tokio::test]
    async fn test_session_lifecycle_manager_reports_compaction_delta() {
        let manager = SessionLifecycleManager::new(
            "liveclaw",
            &MemoryCompactionPolicy {
                enabled: true,
                max_events_threshold: 1,
            },
        )
        .unwrap();

        manager
            .initialize_session("principal-2", "sess-lifecycle-2")
            .await
            .unwrap();
        let compactions = manager
            .record_transcript("principal-2", "sess-lifecycle-2", "compact me")
            .await
            .unwrap();
        assert!(
            compactions > 0,
            "expected at least one compaction event when compaction interval is 1"
        );

        let snapshot = manager
            .snapshot_memory_entries("principal-2", "sess-lifecycle-2")
            .await
            .unwrap();
        assert!(!snapshot.is_empty(), "expected compaction summary entry");
        let summary = snapshot[0]
            .content
            .parts
            .iter()
            .find_map(Part::text)
            .unwrap_or_default();
        assert!(summary.contains("compaction_summary"));
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

    #[test]
    fn test_validate_runtime_and_provider_docker_reports_runtime_image_note() {
        let mut cfg = LiveClawConfig::default();
        cfg.runtime.kind = RuntimeKind::Docker;
        cfg.runtime.docker_image = "zavoraai/liveclaw-runtime:test".to_string();

        let (_provider, report) = validate_runtime_and_provider(&cfg, false).unwrap();
        assert!(
            report
                .notes
                .iter()
                .any(|note| note.contains("runtime.docker_image=zavoraai/liveclaw-runtime:test")),
            "expected docker image diagnostic note, got {:?}",
            report.notes
        );
    }

    #[test]
    fn test_build_docker_runtime_command_includes_worker_mode_and_env() {
        let cfg = DockerRuntimeConfig {
            session_id: "sess docker 42".to_string(),
            image: "zavoraai/liveclaw-runtime:test".to_string(),
            api_key: "secret".to_string(),
            model: "gpt-4o-realtime-preview-2024-12-17".to_string(),
            voice: "alloy".to_string(),
            instructions: Some("You are a docker runtime worker.".to_string()),
            base_url: Some("wss://api.openai.com/v1/realtime".to_string()),
        };

        let args = DockerRuntimeBridgeRuntime::docker_runtime_command_args(&cfg);
        assert!(args.contains(&"run".to_string()));
        assert!(args.contains(&"--runtime-worker".to_string()));
        assert!(args.contains(&cfg.image));
        assert!(
            args.iter()
                .any(|arg| arg.starts_with("LIVECLAW_RUNTIME_API_KEY=")),
            "expected API key env argument in docker run args: {:?}",
            args
        );
        assert!(
            args.iter()
                .any(|arg| arg.starts_with("LIVECLAW_RUNTIME_MODEL=")),
            "expected model env argument in docker run args: {:?}",
            args
        );
        assert!(
            args.iter()
                .any(|arg| arg.starts_with("LIVECLAW_RUNTIME_BASE_URL=")),
            "expected base URL env argument in docker run args: {:?}",
            args
        );
    }

    #[test]
    fn test_parse_onboard_args_supports_force_and_service_flags() {
        let args = vec![
            "config/liveclaw.toml".to_string(),
            "--force".to_string(),
            "--install-service".to_string(),
            "--start-service".to_string(),
            "--gateway-port".to_string(),
            "9001".to_string(),
            "--no-pairing".to_string(),
        ];

        let command = parse_onboard_args(&args).unwrap();
        let AppCommand::Onboard(opts) = command else {
            panic!("expected onboard command");
        };
        assert_eq!(opts.config_path, "config/liveclaw.toml");
        assert!(opts.force);
        assert!(opts.install_service);
        assert!(opts.start_service);
        assert_eq!(opts.gateway_port, Some(9001));
        assert_eq!(opts.require_pairing, Some(false));
    }

    #[test]
    fn test_parse_service_install_args_supports_config_and_force() {
        let args = vec![
            "install".to_string(),
            "--config".to_string(),
            "liveclaw.toml".to_string(),
            "--force".to_string(),
        ];

        let command = parse_service_args(&args).unwrap();
        assert_eq!(
            command,
            AppCommand::Service(ServiceCommand::Install {
                config_path: "liveclaw.toml".to_string(),
                force: true
            })
        );
    }

    #[test]
    fn test_parse_service_logs_args_supports_lines_follow_and_stream() {
        let args = vec![
            "logs".to_string(),
            "--lines".to_string(),
            "120".to_string(),
            "--follow".to_string(),
            "--stream".to_string(),
            "stderr".to_string(),
        ];

        let command = parse_service_args(&args).unwrap();
        assert_eq!(
            command,
            AppCommand::Service(ServiceCommand::Logs {
                lines: 120,
                follow: true,
                stream: ServiceLogStream::Stderr,
            })
        );
    }

    #[test]
    fn test_parse_service_doctor_alias_supports_config() {
        let args = vec![
            "doctor".to_string(),
            "--config".to_string(),
            "config/liveclaw.toml".to_string(),
        ];

        let command = parse_service_args(&args).unwrap();
        assert_eq!(
            command,
            AppCommand::Doctor {
                config_path: "config/liveclaw.toml".to_string(),
            }
        );
    }

    #[test]
    fn test_parse_service_logs_rejects_zero_lines() {
        let args = vec!["logs".to_string(), "--lines".to_string(), "0".to_string()];
        let err = parse_service_args(&args).unwrap_err();
        assert!(err.to_string().contains("greater than zero"));
    }

    #[test]
    fn test_build_onboard_config_non_interactive_requires_api_key() {
        let opts = OnboardOptions {
            non_interactive: true,
            ..OnboardOptions::default()
        };
        let previous_liveclaw_key = std::env::var("LIVECLAW_API_KEY").ok();
        let previous_openai_key = std::env::var("OPENAI_API_KEY").ok();
        std::env::remove_var("LIVECLAW_API_KEY");
        std::env::remove_var("OPENAI_API_KEY");

        let result = build_onboard_config(&opts);

        match previous_liveclaw_key {
            Some(value) => std::env::set_var("LIVECLAW_API_KEY", value),
            None => std::env::remove_var("LIVECLAW_API_KEY"),
        }
        match previous_openai_key {
            Some(value) => std::env::set_var("OPENAI_API_KEY", value),
            None => std::env::remove_var("OPENAI_API_KEY"),
        }

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Missing API key"));
    }

    #[test]
    fn test_build_onboard_config_non_interactive_openai_compatible() {
        let opts = OnboardOptions {
            non_interactive: true,
            api_key: Some("compat-key".to_string()),
            model: Some("gpt-4o-realtime-preview-2024-12-17".to_string()),
            provider_profile: Some("openai_compatible".to_string()),
            base_url: Some("wss://compat.example/v1/realtime".to_string()),
            gateway_host: Some("127.0.0.1".to_string()),
            gateway_port: Some(8420),
            require_pairing: Some(true),
            ..OnboardOptions::default()
        };

        let config = build_onboard_config(&opts).unwrap();
        assert_eq!(config.providers.active_profile, "openai_compatible");
        assert!(config.providers.openai_compatible.enabled);
        assert_eq!(
            config.providers.openai_compatible.base_url.as_deref(),
            Some("wss://compat.example/v1/realtime")
        );
        assert_eq!(config.voice.provider, "openai_compatible");
    }

    #[test]
    fn test_write_onboard_config_refuses_overwrite_without_force() {
        let path = std::env::temp_dir().join(format!(
            "liveclaw-onboard-write-{}-{}.toml",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));

        let config = LiveClawConfig::default();
        write_onboard_config(path.to_str().unwrap(), &config, true).unwrap();
        let result = write_onboard_config(path.to_str().unwrap(), &config, false);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Refusing to overwrite"));
        let _ = std::fs::remove_file(path);
    }
}
