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
use adk_memory::MemoryService;
use adk_realtime::config::ToolDefinition;
use adk_realtime::error::RealtimeError;
use adk_realtime::events::ToolCall;
use adk_realtime::openai::OpenAIRealtimeModel;
use adk_realtime::runner::{EventHandler, ToolHandler};
use adk_realtime::{RealtimeConfig, RealtimeRunner};
use adk_telemetry::{init_telemetry, init_with_otlp};

use liveclaw_app::config::{
    CompactionConfig, LiveClawConfig, ResilienceConfig, SecurityConfig as AppSecurityConfig,
    VoiceConfig,
};
use liveclaw_app::storage::{build_memory_service, FileArtifactService};
use liveclaw_app::tools::build_baseline_tools;
use liveclaw_gateway::pairing::PairingGuard;
use liveclaw_gateway::protocol::SessionConfig;
use liveclaw_gateway::server::{
    Gateway, GatewayConfig, RunnerHandle, SessionAudioOutput, SessionTranscriptOutput,
};

/// A connected realtime session and its background event loop task.
struct RealtimeSession {
    runtime: Arc<dyn SessionRuntime>,
    run_task: JoinHandle<()>,
    user_id: String,
    shutdown: Arc<AtomicBool>,
}

#[async_trait]
trait SessionRuntime: Send + Sync {
    async fn connect(&self) -> Result<()>;
    async fn run(&self) -> Result<()>;
    async fn close(&self) -> Result<()>;
    async fn send_audio_base64(&self, audio_base64: &str) -> Result<()>;
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

#[derive(Clone)]
struct ToolSecurityConfig {
    default_role: String,
    tool_allowlist: Vec<String>,
    audit_log_path: String,
}

impl From<&AppSecurityConfig> for ToolSecurityConfig {
    fn from(value: &AppSecurityConfig) -> Self {
        Self {
            default_role: value.default_role.clone(),
            tool_allowlist: value.tool_allowlist.clone(),
            audit_log_path: value.audit_log_path.clone(),
        }
    }
}

#[derive(Default)]
struct ToolExecutionMetrics {
    total_calls: AtomicU64,
    failed_calls: AtomicU64,
    total_duration_millis: AtomicU64,
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
    policy: &ReconnectPolicy,
) -> bool {
    if !policy.enable_reconnect || policy.max_attempts == 0 {
        return false;
    }

    for attempt in 1..=policy.max_attempts {
        if shutdown.load(Ordering::SeqCst) {
            return false;
        }

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
                info!(
                    session_id = session_id,
                    attempt = attempt,
                    "Realtime reconnect succeeded"
                );
                return true;
            }
            Err(e) => {
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
    provider: String,
    api_key: String,
    default_model: String,
    default_voice: Option<String>,
    default_instructions: Option<String>,
    tool_security: ToolSecurityConfig,
    tool_metrics: Arc<ToolExecutionMetrics>,
    memory_store: Arc<dyn MemoryService>,
    session_memory_entries: Arc<RwLock<HashMap<String, Vec<adk_memory::MemoryEntry>>>>,
    artifact_service: Option<Arc<dyn ArtifactService>>,
    reconnect_policy: ReconnectPolicy,
    memory_compaction_policy: MemoryCompactionPolicy,
    sessions: Arc<RwLock<HashMap<String, RealtimeSession>>>,
    audio_tx: mpsc::Sender<SessionAudioOutput>,
    transcript_tx: mpsc::Sender<SessionTranscriptOutput>,
}

struct RunnerAdapterInit {
    memory_store: Arc<dyn MemoryService>,
    artifact_service: Option<Arc<dyn ArtifactService>>,
    reconnect_policy: ReconnectPolicy,
    memory_compaction_policy: MemoryCompactionPolicy,
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
            provider: voice_cfg.provider.clone(),
            api_key: effective_api_key(&voice_cfg.api_key),
            default_model: voice_cfg.model.clone(),
            default_voice: voice_cfg.voice.clone(),
            default_instructions: voice_cfg.instructions.clone(),
            tool_security: ToolSecurityConfig::from(security_cfg),
            tool_metrics: Arc::new(ToolExecutionMetrics::default()),
            memory_store: init.memory_store,
            session_memory_entries: Arc::new(RwLock::new(HashMap::new())),
            artifact_service: init.artifact_service,
            reconnect_policy: init.reconnect_policy,
            memory_compaction_policy: init.memory_compaction_policy,
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
            .or_else(|| non_empty(Some(self.default_model.clone())))
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
        Ok(middleware.protect_all(build_baseline_tools()))
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
        if !self.provider.eq_ignore_ascii_case("openai") {
            bail!(
                "Unsupported voice provider '{}'. Current runtime supports only 'openai'.",
                self.provider
            );
        }

        if self.api_key.trim().is_empty() {
            bail!("Missing API key. Set [voice].api_key or LIVECLAW_API_KEY / OPENAI_API_KEY.");
        }

        if let Some(cfg) = &config {
            if cfg.enable_graph.is_some() {
                info!(
                    session_id = session_id,
                    "Ignoring enable_graph override in realtime runner path"
                );
            }
        }

        let (model_id, voice, instructions) = self.resolve_session_settings(config.as_ref());
        let role = self.resolve_role(config.as_ref())?;
        let protected_tools = self.build_protected_tools(user_id, &role)?;
        info!(
            session_id = session_id,
            role = %role,
            tool_count = protected_tools.len(),
            "Configured baseline toolset for realtime session"
        );

        // Build adk-realtime runner for this session.
        let model = OpenAIRealtimeModel::new(self.api_key.clone(), model_id.clone());
        let mut realtime_config = RealtimeConfig::default()
            .with_model(model_id)
            .with_text_and_audio()
            .with_server_vad()
            .with_voice(voice);

        if let Some(instr) = instructions {
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
                audio_sequence: AtomicU64::new(0),
                transcript_sequence: AtomicU64::new(0),
                audio_tx: self.audio_tx.clone(),
                transcript_tx: self.transcript_tx.clone(),
            });

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

        let runtime = {
            let sessions = self.sessions.read().await;
            sessions.get(session_id).map(|s| s.runtime.clone())
        }
        .ok_or_else(|| anyhow::anyhow!("Session '{}' not found", session_id))?;

        let audio_b64 = base64_encode(audio);
        runtime.send_audio_base64(&audio_b64).await
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
    // Parse config path from CLI args (first arg) or default to "liveclaw.toml"
    let config_path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "liveclaw.toml".to_string());

    let config = LiveClawConfig::load(&config_path)?;

    // Initialize telemetry
    if config.telemetry.otlp_enabled {
        init_with_otlp("liveclaw", "http://localhost:4317").ok();
    } else {
        init_telemetry("liveclaw").ok();
    }
    info!("LiveClaw starting with config from '{}'", config_path);

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
            memory_store,
            artifact_service,
            reconnect_policy: ReconnectPolicy::from(&config.resilience),
            memory_compaction_policy: MemoryCompactionPolicy::from(&config.compaction),
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

    fn test_adapter(default_role: &str, allowlist: Vec<&str>) -> RunnerAdapter {
        let voice_cfg = VoiceConfig {
            provider: "openai".to_string(),
            api_key: "test-key".to_string(),
            model: "gpt-4o-realtime-preview-2024-12-17".to_string(),
            voice: Some("alloy".to_string()),
            instructions: Some("test".to_string()),
            audio_format: liveclaw_app::config::AudioFormat::Pcm16_24kHz,
        };

        let security_cfg = AppSecurityConfig {
            default_role: default_role.to_string(),
            tool_allowlist: allowlist.into_iter().map(str::to_string).collect(),
            rate_limit_per_session: 100,
            audit_log_path: "/tmp/liveclaw-test-audit.jsonl".to_string(),
        };

        let (audio_tx, _audio_rx) = mpsc::channel(4);
        let (transcript_tx, _transcript_rx) = mpsc::channel(4);
        let memory_store: Arc<dyn MemoryService> =
            Arc::new(adk_memory::InMemoryMemoryService::new());

        RunnerAdapter::new(
            &voice_cfg,
            &security_cfg,
            RunnerAdapterInit {
                memory_store,
                artifact_service: None,
                reconnect_policy: ReconnectPolicy::from(&ResilienceConfig::default()),
                memory_compaction_policy: MemoryCompactionPolicy::from(&CompactionConfig::default()),
                audio_tx,
                transcript_tx,
            },
        )
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
        let adapter = test_adapter("supervised", vec!["echo_text", "add_numbers", "utc_time"]);
        let tools = adapter
            .build_protected_tools("principal-1", "supervised")
            .unwrap();
        assert_eq!(tools.len(), 3);
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

        let handle =
            spawn_runtime_loop("sess-budget".to_string(), runtime.clone(), shutdown, policy);
        tokio::time::timeout(Duration::from_millis(200), handle)
            .await
            .expect("runtime loop should complete")
            .expect("runtime task should not panic");

        assert_eq!(runtime.run_calls.load(Ordering::SeqCst), 1);
        assert_eq!(runtime.connect_calls.load(Ordering::SeqCst), 2);
    }
}
