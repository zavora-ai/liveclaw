use std::sync::Arc;

use anyhow::Result;
use tokio::sync::mpsc;

use adk_auth::{AccessControl, AuthMiddleware};
use adk_realtime::RealtimeAgent;

use crate::security::{contains_shell_injection, RateLimiter};

// ---------------------------------------------------------------------------
// Output types for callback channels (Gateway ↔ RealtimeAgent)
// ---------------------------------------------------------------------------

/// Audio data forwarded from the RealtimeAgent to the Gateway for client playback.
pub struct AudioOutput {
    pub data: Vec<u8>,
}

/// Transcript update forwarded from the RealtimeAgent to the Gateway.
pub struct TranscriptOutput {
    pub text: String,
    pub is_final: bool,
}

// ---------------------------------------------------------------------------
// VoiceConfig
// ---------------------------------------------------------------------------

/// Voice provider configuration.
pub struct VoiceConfig {
    pub provider: String,
    pub api_key: String,
    pub model: String,
    pub voice: Option<String>,
    pub instructions: Option<String>,
    pub audio_format: String,
}

// ---------------------------------------------------------------------------
// RealtimeAgent construction
// ---------------------------------------------------------------------------

/// Build a [`RealtimeAgent`] wired with all callbacks and protected tools.
pub fn build_realtime_agent(
    config: &VoiceConfig,
    tools: Vec<Arc<dyn adk_core::Tool>>,
    access_control: Arc<AccessControl>,
    rate_limiter: Arc<RateLimiter>,
    audio_tx: mpsc::Sender<AudioOutput>,
    transcript_tx: mpsc::Sender<TranscriptOutput>,
) -> Result<RealtimeAgent> {
    // Wrap tools with AuthMiddleware for role-based access control
    let ac = Arc::try_unwrap(access_control).unwrap_or_else(|arc| (*arc).clone());
    let middleware = AuthMiddleware::new(ac);
    let protected_tools = middleware.protect_all(tools);

    let mut builder = RealtimeAgent::builder("voice_assistant")
        .instruction(&config.instructions.clone().unwrap_or_default())
        .voice(&config.voice.clone().unwrap_or_else(|| "alloy".into()))
        .server_vad()
        // before_tool_callback: rate limiting + shell injection detection
        // Signature: Fn(Arc<dyn CallbackContext>) -> Future<Result<Option<Content>>>
        .before_tool_callback(Box::new(move |ctx| {
            let rl = rate_limiter.clone();
            Box::pin(async move {
                let session_id = ctx.session_id();
                if !rl.check_and_increment(session_id) {
                    return Err(adk_core::AdkError::Agent(format!(
                        "Rate limit exceeded for session {}",
                        session_id
                    )));
                }
                // Shell injection check on user content
                let content = ctx.user_content();
                for part in &content.parts {
                    if let Some(text) = part.text() {
                        if contains_shell_injection(text) {
                            return Err(adk_core::AdkError::Agent(
                                "Shell injection detected in tool arguments".to_string(),
                            ));
                        }
                    }
                }
                Ok(None)
            })
        }))
        // after_tool_callback: log tool execution via tracing
        .after_tool_callback(Box::new(move |ctx| {
            Box::pin(async move {
                tracing::info!(
                    agent = %ctx.agent_name(),
                    session = %ctx.session_id(),
                    "Tool execution completed"
                );
                Ok(None)
            })
        }))
        // on_audio: forward audio output to Gateway (bytes, session_id)
        .on_audio(Arc::new(move |audio_data: &[u8], _session_id: &str| {
            let tx = audio_tx.clone();
            let data = audio_data.to_vec();
            Box::pin(async move {
                let _ = tx.send(AudioOutput { data }).await;
            })
        }))
        // on_transcript: forward transcript updates (text, session_id)
        .on_transcript(Arc::new(move |text: &str, _session_id: &str| {
            let tx = transcript_tx.clone();
            let t = text.to_string();
            Box::pin(async move {
                let _ = tx.send(TranscriptOutput {
                    text: t,
                    is_final: true,
                }).await;
            })
        }))
        // on_speech_started: latency tracking (timestamp_ms)
        .on_speech_started(Arc::new(move |ts: u64| {
            Box::pin(async move {
                tracing::debug!(timestamp_ms = ts, "Speech started");
            })
        }))
        // on_speech_stopped: latency tracking (timestamp_ms)
        .on_speech_stopped(Arc::new(move |ts: u64| {
            Box::pin(async move {
                tracing::debug!(timestamp_ms = ts, "Speech stopped");
            })
        }));

    // Register all protected tools
    for tool in protected_tools {
        builder = builder.tool(tool);
    }

    builder.build().map_err(|e| anyhow::anyhow!("{}", e))
}
