//! LiveClaw entry point — wires all ADK-Rust components together and starts
//! the Gateway server.

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use tokio::sync::mpsc;
use tracing::info;

use adk_memory::InMemoryMemoryService;
use adk_runner::EventsCompactionConfig;
use adk_session::InMemorySessionService;
use adk_telemetry::{init_telemetry, init_with_otlp};

use liveclaw_app::agent::{self, AudioOutput, TranscriptOutput};
use liveclaw_app::config::LiveClawConfig;
use liveclaw_app::graph;
use liveclaw_app::memory::MemoryAdapter;
use liveclaw_app::plugins::{build_pii_patterns, build_plugin_manager};
use liveclaw_app::runner::build_runner;
use liveclaw_app::security::{self, RateLimiter};

use liveclaw_gateway::pairing::PairingGuard;
use liveclaw_gateway::server::{
    Gateway, GatewayConfig, SessionAudioOutput, SessionTranscriptOutput,
};

// ---------------------------------------------------------------------------
// RunnerHandle adapter — bridges adk-runner::Runner to the Gateway's trait
// ---------------------------------------------------------------------------

use async_trait::async_trait;
use liveclaw_gateway::protocol::SessionConfig;
use liveclaw_gateway::server::RunnerHandle;

/// Adapter that implements the Gateway's [`RunnerHandle`] trait by delegating
/// to `adk-runner::Runner`.
struct RunnerAdapter {
    runner: Arc<adk_runner::Runner>,
}

#[async_trait]
impl RunnerHandle for RunnerAdapter {
    async fn create_session(
        &self,
        user_id: &str,
        session_id: &str,
        _config: Option<SessionConfig>,
    ) -> Result<String> {
        // Delegate to Runner — it handles session creation via SessionService
        self.runner
            .run(
                user_id.to_string(),
                session_id.to_string(),
                adk_core::Content::new("user"),
            )
            .await
            .map(|_| session_id.to_string())
            .map_err(|e| anyhow::anyhow!("{}", e))
    }

    async fn terminate_session(&self, _session_id: &str) -> Result<()> {
        // Runner handles cleanup when the event stream ends
        Ok(())
    }

    async fn send_audio(&self, _session_id: &str, _audio: &[u8]) -> Result<()> {
        // Audio forwarding is handled via the mpsc channels wired into
        // the RealtimeAgent callbacks
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Placeholder tool loader
// ---------------------------------------------------------------------------

/// Placeholder tool loader — returns an empty Vec.
/// Actual tool loading depends on the specific deployment configuration.
fn load_tools() -> Vec<Arc<dyn adk_core::Tool>> {
    Vec::new()
}

// ---------------------------------------------------------------------------
// Application entry point
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> Result<()> {
    // Parse config path from CLI args (first arg) or default to "liveclaw.toml"
    let config_path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "liveclaw.toml".to_string());

    let config = LiveClawConfig::load(&config_path)?;

    // 1. Initialize telemetry
    if config.telemetry.otlp_enabled {
        init_with_otlp("liveclaw", "http://localhost:4317").ok();
    } else {
        init_telemetry("liveclaw").ok();
    }
    info!("LiveClaw starting with config from '{}'", config_path);

    // 2. Build adk-auth AccessControl
    let security_cfg = security::SecurityConfig {
        default_role: config.security.default_role.clone(),
        tool_allowlist: config.security.tool_allowlist.clone(),
        rate_limit_per_session: config.security.rate_limit_per_session,
        audit_log_path: config.security.audit_log_path.clone(),
    };
    let access_control = Arc::new(security::build_access_control(&security_cfg));

    // 3. Build rate limiter
    let rate_limiter = Arc::new(RateLimiter::new(config.security.rate_limit_per_session));

    // 4. Build audio/transcript channels for Gateway ↔ RealtimeAgent
    //
    // Agent-side channels: the RealtimeAgent callbacks produce un-tagged
    // AudioOutput/TranscriptOutput. These are consumed by the agent.
    let (audio_tx, _audio_rx) = mpsc::channel::<AudioOutput>(256);
    let (transcript_tx, _transcript_rx) = mpsc::channel::<TranscriptOutput>(256);

    // Gateway-side channels: session-tagged outputs that the Gateway consumes
    // to forward AudioOutput and TranscriptUpdate responses to WebSocket clients.
    let (gw_audio_tx, gw_audio_rx) = mpsc::channel::<SessionAudioOutput>(256);
    let (gw_transcript_tx, gw_transcript_rx) = mpsc::channel::<SessionTranscriptOutput>(256);

    // Bridge: spawn tasks that read from agent-side channels and forward to
    // gateway-side channels with session tagging. In a full implementation,
    // the session_id would come from the InvocationContext in the agent
    // callbacks. For now, we use a placeholder that will be replaced when
    // per-session callback wiring is implemented.
    {
        let gw_audio_tx = gw_audio_tx.clone();
        tokio::spawn(async move {
            let mut rx = _audio_rx;
            while let Some(output) = rx.recv().await {
                let _ = gw_audio_tx
                    .send(SessionAudioOutput {
                        session_id: "default".to_string(),
                        data: output.data,
                    })
                    .await;
            }
        });
    }
    {
        let gw_transcript_tx = gw_transcript_tx.clone();
        tokio::spawn(async move {
            let mut rx = _transcript_rx;
            while let Some(output) = rx.recv().await {
                let _ = gw_transcript_tx
                    .send(SessionTranscriptOutput {
                        session_id: "default".to_string(),
                        text: output.text,
                        is_final: output.is_final,
                    })
                    .await;
            }
        });
    }

    // 5. Build tools and RealtimeAgent
    let tools = load_tools();
    let voice_cfg = agent::VoiceConfig {
        provider: config.voice.provider.clone(),
        api_key: config.voice.api_key.clone(),
        model: config.voice.model.clone(),
        voice: config.voice.voice.clone(),
        instructions: config.voice.instructions.clone(),
        audio_format: format!("{:?}", config.voice.audio_format),
    };
    let realtime_agent = agent::build_realtime_agent(
        &voice_cfg,
        tools,
        access_control.clone(),
        rate_limiter.clone(),
        audio_tx,
        transcript_tx,
    )?;

    // 6. Optionally wrap in GraphAgent
    let agent: Arc<dyn adk_core::Agent> = if config.graph.enable_graph {
        let graph_cfg = graph::GraphConfig {
            enable_graph: config.graph.enable_graph,
            recursion_limit: Some(config.graph.recursion_limit),
        };
        Arc::new(graph::build_graph_agent(
            realtime_agent,
            &graph_cfg,
            &config.security.default_role,
        )?)
    } else {
        Arc::new(realtime_agent)
    };

    // 7. Build SessionService
    let session_service: Arc<dyn adk_session::SessionService> =
        Arc::new(InMemorySessionService::new());

    // 8. Build MemoryStore and MemoryAdapter
    let memory_store: Arc<dyn adk_memory::MemoryService> = Arc::new(InMemoryMemoryService::new());
    let memory_adapter = Arc::new(MemoryAdapter::new(
        memory_store.clone(),
        config.memory.recall_limit,
    ));

    // 9. Build artifact service (optional)
    let artifact_service: Option<Arc<dyn adk_artifact::ArtifactService>> =
        if config.artifact.enable_artifacts {
            Some(Arc::new(adk_artifact::InMemoryArtifactService::new()))
        } else {
            None
        };

    // 10. Build PluginManager
    let pii_patterns = build_pii_patterns();
    let plugin_manager = Arc::new(build_plugin_manager(
        memory_store.clone(),
        pii_patterns,
        config.plugin.custom_blocked_keywords.clone(),
    ));

    // 11. Build compaction config (optional — requires a summarizer implementation)
    let compaction_config: Option<EventsCompactionConfig> = None;

    // 12. Build Runner
    let runner = Arc::new(build_runner(
        agent,
        session_service,
        Some(memory_adapter),
        artifact_service,
        Some(plugin_manager),
        compaction_config,
    )?);

    // 13. Build PairingGuard
    let pairing = Arc::new(PairingGuard::with_lockout(
        config.gateway.require_pairing,
        &[],
        config.pairing.max_attempts,
        Duration::from_secs(config.pairing.lockout_duration_secs),
    ));

    // 14. Build and start Gateway
    let gateway_config = GatewayConfig {
        host: config.gateway.host.clone(),
        port: config.gateway.port,
    };
    let runner_handle: Arc<dyn RunnerHandle> = Arc::new(RunnerAdapter { runner });
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
