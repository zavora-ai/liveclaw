use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use axum::extract::ws::{Message, WebSocket};
use axum::extract::{Json as JsonExtract, Query, State, WebSocketUpgrade};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Json};
use axum::routing::{get, post};
use axum::Router;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::net::TcpListener;
use tokio::sync::{mpsc, oneshot, RwLock};
use tokio::time::{self, Duration, MissedTickBehavior};
use tracing::{error, info, warn};

use crate::pairing::PairingGuard;
use crate::protocol::{
    ChannelJob, ChannelOutboundItem, GatewayHealth, GatewayMessage, GatewayResponse,
    GraphExecutionReport, PriorityNotice, RuntimeDiagnostics, SessionConfig, PROTOCOL_VERSION,
};

// ---------------------------------------------------------------------------
// RunnerHandle trait — abstracts the Runner interface so the gateway crate
// doesn't depend on adk-runner directly.
// ---------------------------------------------------------------------------

/// Trait abstracting the Runner interface for session lifecycle management.
///
/// The `liveclaw-app` crate provides a concrete implementation that delegates
/// to `adk-runner::Runner`.
#[async_trait]
pub trait RunnerHandle: Send + Sync {
    /// Create a new voice session and return the assigned session ID.
    async fn create_session(
        &self,
        user_id: &str,
        session_id: &str,
        config: Option<SessionConfig>,
    ) -> anyhow::Result<String>;

    /// Terminate an active voice session and release its resources.
    async fn terminate_session(&self, session_id: &str) -> anyhow::Result<()>;

    /// Forward audio data to the active RealtimeAgent session.
    async fn send_audio(&self, session_id: &str, audio: &[u8]) -> anyhow::Result<()>;

    /// Commit buffered audio for manual turn-management flows.
    async fn commit_audio(&self, session_id: &str) -> anyhow::Result<()>;

    /// Trigger model response generation for manual turn-management flows.
    async fn create_response(&self, session_id: &str) -> anyhow::Result<()>;

    /// Interrupt an in-flight model response.
    async fn interrupt_response(&self, session_id: &str) -> anyhow::Result<()>;

    /// Send a prompt text into the session conversation.
    async fn send_text(&self, session_id: &str, prompt: &str) -> anyhow::Result<()>;

    /// Execute a tool call through the runtime graph path for deterministic
    /// M3 validation.
    async fn session_tool_call(
        &self,
        session_id: &str,
        tool_name: &str,
        arguments: serde_json::Value,
    ) -> anyhow::Result<(serde_json::Value, GraphExecutionReport)>;

    /// Return runtime diagnostics for operator validation.
    async fn diagnostics(&self) -> anyhow::Result<RuntimeDiagnostics>;
}

// ---------------------------------------------------------------------------
// Session-tagged output types for channel wiring
// ---------------------------------------------------------------------------

/// Audio output tagged with a session ID, received from the RealtimeAgent
/// callbacks and forwarded to the WebSocket client as `AudioOutput` responses.
pub struct SessionAudioOutput {
    pub session_id: String,
    pub data: Vec<u8>,
}

/// Transcript output tagged with a session ID, received from the RealtimeAgent
/// callbacks and forwarded to the WebSocket client as `TranscriptUpdate` responses.
pub struct SessionTranscriptOutput {
    pub session_id: String,
    pub text: String,
    pub is_final: bool,
}

// ---------------------------------------------------------------------------
// GatewayConfig
// ---------------------------------------------------------------------------

/// Configuration for the Gateway server.
#[derive(Debug, Clone)]
pub struct GatewayConfig {
    pub host: String,
    pub port: u16,
}

impl Default for GatewayConfig {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".to_string(),
            port: 8420,
        }
    }
}

// ---------------------------------------------------------------------------
// Shared application state passed to axum handlers
// ---------------------------------------------------------------------------

/// Shared state accessible by all axum handlers and WebSocket connections.
#[derive(Clone)]
struct AppState {
    pairing: Arc<PairingGuard>,
    runner: Arc<dyn RunnerHandle>,
    started_at: Instant,
    audio_senders: Arc<RwLock<HashMap<String, mpsc::Sender<Vec<u8>>>>>,
    /// Stable principal owner for each session ID (supports token-authenticated
    /// reconnect/resume across different WebSocket connections).
    session_owners: Arc<RwLock<HashMap<String, String>>>,
    /// Per-session senders for forwarding GatewayResponse messages (AudioOutput,
    /// TranscriptUpdate) back to the WebSocket connection that owns the session.
    ws_response_senders: Arc<RwLock<HashMap<String, mpsc::Sender<GatewayResponse>>>>,
    /// Per-session senders for forwarding priority control-plane responses
    /// (PriorityNotice) with precedence over standard channel traffic.
    ws_priority_senders: Arc<RwLock<HashMap<String, mpsc::Sender<GatewayResponse>>>>,
    /// Channel/account/user routing table scoped by authenticated principal.
    channel_routes: Arc<RwLock<HashMap<ChannelRouteKey, String>>>,
    /// Reverse index from session ID to channel route metadata.
    session_channel_routes: Arc<RwLock<HashMap<String, ChannelRouteKey>>>,
    /// Buffered outbound events waiting for adapter delivery.
    channel_outbox: Arc<RwLock<HashMap<ChannelRouteKey, Vec<ChannelOutboundItem>>>>,
    /// Active background channel jobs keyed by job id.
    channel_jobs: Arc<RwLock<HashMap<String, ChannelJobRuntime>>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ChannelRouteKey {
    principal_id: String,
    channel: String,
    account_id: String,
    external_user_id: String,
}

struct ChannelInboundRequest<'a> {
    channel: &'a str,
    account_id: &'a str,
    external_user_id: &'a str,
    text: &'a str,
    create_response: bool,
    session_config_on_create: Option<SessionConfig>,
    force_session_recreate: bool,
}

struct ChannelOutboundPollRequest<'a> {
    channel: &'a str,
    account_id: &'a str,
    external_user_id: &'a str,
    max_items: Option<usize>,
}

struct ChannelJobCreateRequest<'a> {
    channel: &'a str,
    account_id: &'a str,
    external_user_id: &'a str,
    text: &'a str,
    interval_seconds: u64,
    create_response: bool,
}

struct ChannelJobRuntime {
    owner_principal_id: String,
    job: ChannelJob,
    cancel_tx: Option<oneshot::Sender<()>>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct ChannelIngressQuery {
    token: Option<String>,
    create_response: Option<bool>,
}

#[derive(Debug, Clone, Deserialize)]
struct WebhookInboundPayload {
    channel: Option<String>,
    account_id: String,
    external_user_id: String,
    text: String,
    create_response: Option<bool>,
}

#[derive(Debug, Clone, Deserialize)]
struct ChannelOutboundPollPayload {
    channel: String,
    account_id: String,
    external_user_id: String,
    max_items: Option<usize>,
}

#[derive(Debug, Clone, Deserialize)]
struct ChannelJobCreatePayload {
    channel: String,
    account_id: String,
    external_user_id: String,
    text: String,
    interval_seconds: u64,
    create_response: Option<bool>,
}

#[derive(Debug, Clone, Deserialize)]
struct ChannelJobCancelPayload {
    job_id: String,
}

#[derive(Debug, Clone, Deserialize)]
struct WebhookSupervisedActionPayload {
    channel: Option<String>,
    account_id: String,
    external_user_id: String,
    text: Option<String>,
    tool_name: String,
    arguments: serde_json::Value,
    create_response: Option<bool>,
    force_session_recreate: Option<bool>,
}

#[derive(Debug, Clone, Deserialize)]
struct SlackEnvelope {
    #[serde(rename = "type")]
    envelope_type: Option<String>,
    challenge: Option<String>,
    team_id: Option<String>,
    event: Option<SlackEvent>,
}

#[derive(Debug, Clone, Deserialize)]
struct SlackEvent {
    #[serde(rename = "type")]
    event_type: Option<String>,
    user: Option<String>,
    text: Option<String>,
    bot_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct TelegramUpdate {
    message: Option<TelegramMessage>,
    edited_message: Option<TelegramMessage>,
    channel_post: Option<TelegramMessage>,
}

#[derive(Debug, Clone, Deserialize)]
struct TelegramMessage {
    text: Option<String>,
    chat: Option<TelegramChat>,
    from: Option<TelegramUser>,
}

#[derive(Debug, Clone, Deserialize)]
struct TelegramChat {
    id: i64,
}

#[derive(Debug, Clone, Deserialize)]
struct TelegramUser {
    id: i64,
}

// ---------------------------------------------------------------------------
// Health check response
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct HealthResponse {
    status: &'static str,
    service: &'static str,
}

// ---------------------------------------------------------------------------
// Gateway
// ---------------------------------------------------------------------------

/// The Gateway WebSocket server.
///
/// Accepts WebSocket connections, authenticates clients via PairingGuard,
/// and routes session commands to the Runner. Consumes audio and transcript
/// output channels to forward `AudioOutput` and `TranscriptUpdate` responses
/// back to the appropriate WebSocket clients.
pub struct Gateway {
    config: GatewayConfig,
    pairing: Arc<PairingGuard>,
    runner: Arc<dyn RunnerHandle>,
    audio_senders: Arc<RwLock<HashMap<String, mpsc::Sender<Vec<u8>>>>>,
    /// Receiver for session-tagged audio output from the RealtimeAgent.
    audio_rx: Option<mpsc::Receiver<SessionAudioOutput>>,
    /// Receiver for session-tagged transcript output from the RealtimeAgent.
    transcript_rx: Option<mpsc::Receiver<SessionTranscriptOutput>>,
}

impl Gateway {
    /// Create a new Gateway instance (without output channel receivers).
    pub fn new(
        config: GatewayConfig,
        pairing: Arc<PairingGuard>,
        runner: Arc<dyn RunnerHandle>,
    ) -> Self {
        Self {
            config,
            pairing,
            runner,
            audio_senders: Arc::new(RwLock::new(HashMap::new())),
            audio_rx: None,
            transcript_rx: None,
        }
    }

    /// Create a new Gateway instance with audio and transcript output receivers.
    ///
    /// The Gateway spawns background tasks that consume these receivers and
    /// forward `AudioOutput` and `TranscriptUpdate` responses to the WebSocket
    /// connections that own the corresponding sessions.
    pub fn with_output_channels(
        config: GatewayConfig,
        pairing: Arc<PairingGuard>,
        runner: Arc<dyn RunnerHandle>,
        audio_rx: mpsc::Receiver<SessionAudioOutput>,
        transcript_rx: mpsc::Receiver<SessionTranscriptOutput>,
    ) -> Self {
        Self {
            config,
            pairing,
            runner,
            audio_senders: Arc::new(RwLock::new(HashMap::new())),
            audio_rx: Some(audio_rx),
            transcript_rx: Some(transcript_rx),
        }
    }

    /// Start the Gateway server, binding to the configured host:port.
    ///
    /// Spawns background tasks to consume audio_rx and transcript_rx channels
    /// and forward responses to the appropriate WebSocket connections.
    /// This method runs until the server is shut down (e.g. via signal).
    pub async fn start(self) -> anyhow::Result<()> {
        let ws_response_senders: Arc<RwLock<HashMap<String, mpsc::Sender<GatewayResponse>>>> =
            Arc::new(RwLock::new(HashMap::new()));
        let ws_priority_senders: Arc<RwLock<HashMap<String, mpsc::Sender<GatewayResponse>>>> =
            Arc::new(RwLock::new(HashMap::new()));

        let state = AppState {
            pairing: self.pairing,
            runner: self.runner,
            started_at: Instant::now(),
            audio_senders: self.audio_senders,
            session_owners: Arc::new(RwLock::new(HashMap::new())),
            ws_response_senders: ws_response_senders.clone(),
            ws_priority_senders,
            channel_routes: Arc::new(RwLock::new(HashMap::new())),
            session_channel_routes: Arc::new(RwLock::new(HashMap::new())),
            channel_outbox: Arc::new(RwLock::new(HashMap::new())),
            channel_jobs: Arc::new(RwLock::new(HashMap::new())),
        };

        // Spawn background task to consume audio output and route to WebSocket clients
        if let Some(mut audio_rx) = self.audio_rx {
            let senders = ws_response_senders.clone();
            tokio::spawn(async move {
                while let Some(output) = audio_rx.recv().await {
                    let resp = GatewayResponse::AudioOutput {
                        session_id: output.session_id.clone(),
                        audio: base64_encode(&output.data),
                    };
                    let senders_guard = senders.read().await;
                    if let Some(tx) = senders_guard.get(&output.session_id) {
                        if tx.send(resp).await.is_err() {
                            warn!("Failed to forward audio to session {}", output.session_id);
                        }
                    }
                }
                info!("Audio output channel closed");
            });
        }

        // Spawn background task to consume transcript output and route to WebSocket clients
        if let Some(mut transcript_rx) = self.transcript_rx {
            let senders = ws_response_senders.clone();
            let transcript_state = state.clone();
            tokio::spawn(async move {
                while let Some(output) = transcript_rx.recv().await {
                    let resp = GatewayResponse::TranscriptUpdate {
                        session_id: output.session_id.clone(),
                        text: output.text.clone(),
                        is_final: output.is_final,
                    };
                    let senders_guard = senders.read().await;
                    if let Some(tx) = senders_guard.get(&output.session_id) {
                        if tx.send(resp).await.is_err() {
                            warn!(
                                "Failed to forward transcript to session {}",
                                output.session_id
                            );
                        }
                    }

                    queue_channel_outbound_transcript(&transcript_state, &output).await;
                }
                info!("Transcript output channel closed");
            });
        }

        let app = build_router(state);

        let addr = format!("{}:{}", self.config.host, self.config.port);
        info!("Gateway listening on {}", addr);

        let listener = TcpListener::bind(&addr).await?;
        axum::serve(listener, app).await?;

        Ok(())
    }
}

fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health_handler))
        .route("/ws", get(ws_upgrade_handler))
        .route("/channels/webhook", post(channel_webhook_handler))
        .route("/channels/slack/events", post(channel_slack_events_handler))
        .route(
            "/channels/telegram/update",
            post(channel_telegram_update_handler),
        )
        .route(
            "/channels/outbound/poll",
            post(channel_outbound_poll_handler),
        )
        .route("/channels/jobs/create", post(channel_job_create_handler))
        .route("/channels/jobs/cancel", post(channel_job_cancel_handler))
        .route("/channels/jobs/list", get(channel_job_list_handler))
        .route(
            "/channels/webhook/supervised-action",
            post(channel_webhook_supervised_action_handler),
        )
        .with_state(state)
}

// ---------------------------------------------------------------------------
// HTTP handlers
// ---------------------------------------------------------------------------

/// Health check endpoint — returns 200 OK with readiness JSON.
async fn health_handler() -> impl IntoResponse {
    Json(HealthResponse {
        status: "ok",
        service: "liveclaw-gateway",
    })
}

/// WebSocket upgrade handler — upgrades the HTTP connection to WebSocket
/// and spawns the per-connection handler.
async fn ws_upgrade_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_ws(socket, state))
}

async fn channel_webhook_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<ChannelIngressQuery>,
    JsonExtract(payload): JsonExtract<WebhookInboundPayload>,
) -> impl IntoResponse {
    let token = extract_channel_token(&headers, &query);
    let create_response = payload
        .create_response
        .or(query.create_response)
        .unwrap_or(true);
    let channel = payload.channel.as_deref().unwrap_or("webhook");

    let resp = handle_channel_http_route(
        ChannelInboundRequest {
            channel,
            account_id: &payload.account_id,
            external_user_id: &payload.external_user_id,
            text: &payload.text,
            create_response,
            session_config_on_create: None,
            force_session_recreate: false,
        },
        token,
        &state,
    )
    .await;
    http_response_from_gateway(resp)
}

async fn channel_slack_events_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<ChannelIngressQuery>,
    JsonExtract(payload): JsonExtract<SlackEnvelope>,
) -> impl IntoResponse {
    if payload.envelope_type.as_deref() == Some("url_verification") {
        return (
            StatusCode::OK,
            Json(serde_json::json!({
                "challenge": payload.challenge.unwrap_or_default(),
            })),
        );
    }

    let token = extract_channel_token(&headers, &query);
    let Some(event) = payload.event.as_ref() else {
        return http_json_error(
            StatusCode::BAD_REQUEST,
            "invalid_slack_payload",
            "Slack event payload is missing 'event'",
        );
    };
    if event.event_type.as_deref() != Some("message") {
        return (
            StatusCode::ACCEPTED,
            Json(serde_json::json!({
                "status": "ignored",
                "reason": "unsupported_event_type",
            })),
        );
    }
    if event.bot_id.is_some() {
        return (
            StatusCode::ACCEPTED,
            Json(serde_json::json!({
                "status": "ignored",
                "reason": "bot_message",
            })),
        );
    }

    let Some(account_id) = payload.team_id.as_deref() else {
        return http_json_error(
            StatusCode::BAD_REQUEST,
            "invalid_slack_payload",
            "Slack event payload is missing 'team_id'",
        );
    };
    let Some(external_user_id) = event.user.as_deref() else {
        return http_json_error(
            StatusCode::BAD_REQUEST,
            "invalid_slack_payload",
            "Slack message payload is missing 'event.user'",
        );
    };
    let Some(text) = event.text.as_deref() else {
        return http_json_error(
            StatusCode::BAD_REQUEST,
            "invalid_slack_payload",
            "Slack message payload is missing 'event.text'",
        );
    };

    let resp = handle_channel_http_route(
        ChannelInboundRequest {
            channel: "slack",
            account_id,
            external_user_id,
            text,
            create_response: query.create_response.unwrap_or(true),
            session_config_on_create: None,
            force_session_recreate: false,
        },
        token,
        &state,
    )
    .await;
    http_response_from_gateway(resp)
}

async fn channel_telegram_update_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<ChannelIngressQuery>,
    JsonExtract(payload): JsonExtract<TelegramUpdate>,
) -> impl IntoResponse {
    let token = extract_channel_token(&headers, &query);
    let msg = payload
        .message
        .as_ref()
        .or(payload.edited_message.as_ref())
        .or(payload.channel_post.as_ref());
    let Some(msg) = msg else {
        return (
            StatusCode::ACCEPTED,
            Json(serde_json::json!({
                "status": "ignored",
                "reason": "no_message",
            })),
        );
    };
    let Some(text) = msg.text.as_deref() else {
        return (
            StatusCode::ACCEPTED,
            Json(serde_json::json!({
                "status": "ignored",
                "reason": "no_text",
            })),
        );
    };
    let Some(chat) = msg.chat.as_ref() else {
        return http_json_error(
            StatusCode::BAD_REQUEST,
            "invalid_telegram_payload",
            "Telegram payload is missing 'message.chat.id'",
        );
    };
    let Some(from) = msg.from.as_ref() else {
        return http_json_error(
            StatusCode::BAD_REQUEST,
            "invalid_telegram_payload",
            "Telegram payload is missing 'message.from.id'",
        );
    };

    let account_id = chat.id.to_string();
    let external_user_id = from.id.to_string();
    let resp = handle_channel_http_route(
        ChannelInboundRequest {
            channel: "telegram",
            account_id: &account_id,
            external_user_id: &external_user_id,
            text,
            create_response: query.create_response.unwrap_or(true),
            session_config_on_create: None,
            force_session_recreate: false,
        },
        token,
        &state,
    )
    .await;
    http_response_from_gateway(resp)
}

async fn channel_outbound_poll_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<ChannelIngressQuery>,
    JsonExtract(payload): JsonExtract<ChannelOutboundPollPayload>,
) -> impl IntoResponse {
    let token = extract_channel_token(&headers, &query);
    let resp = handle_channel_http_outbound_poll(
        ChannelOutboundPollRequest {
            channel: &payload.channel,
            account_id: &payload.account_id,
            external_user_id: &payload.external_user_id,
            max_items: payload.max_items,
        },
        token.as_deref(),
        &state,
    )
    .await;
    http_response_from_gateway(resp)
}

async fn channel_job_create_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<ChannelIngressQuery>,
    JsonExtract(payload): JsonExtract<ChannelJobCreatePayload>,
) -> impl IntoResponse {
    let token = extract_channel_token(&headers, &query);
    let resp = handle_channel_http_create_job(
        ChannelJobCreateRequest {
            channel: &payload.channel,
            account_id: &payload.account_id,
            external_user_id: &payload.external_user_id,
            text: &payload.text,
            interval_seconds: payload.interval_seconds,
            create_response: payload.create_response.unwrap_or(true),
        },
        token,
        &state,
    )
    .await;
    http_response_from_gateway(resp)
}

async fn channel_job_cancel_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<ChannelIngressQuery>,
    JsonExtract(payload): JsonExtract<ChannelJobCancelPayload>,
) -> impl IntoResponse {
    let token = extract_channel_token(&headers, &query);
    let resp = handle_channel_http_cancel_job(&payload.job_id, token, &state).await;
    http_response_from_gateway(resp)
}

async fn channel_job_list_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<ChannelIngressQuery>,
) -> impl IntoResponse {
    let token = extract_channel_token(&headers, &query);
    let resp = handle_channel_http_list_jobs(token, &state).await;
    http_response_from_gateway(resp)
}

async fn channel_webhook_supervised_action_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<ChannelIngressQuery>,
    JsonExtract(payload): JsonExtract<WebhookSupervisedActionPayload>,
) -> impl IntoResponse {
    let token = extract_channel_token(&headers, &query);
    let channel = payload.channel.as_deref().unwrap_or("webhook");
    let text = payload
        .text
        .as_deref()
        .unwrap_or("Webhook supervised action trigger");
    let create_response = payload
        .create_response
        .or(query.create_response)
        .unwrap_or(false);
    let force_session_recreate = payload.force_session_recreate.unwrap_or(true);

    let principal_id = match authenticate_http_principal(token.as_deref(), &state) {
        Ok(id) => id,
        Err(resp) => return http_response_from_gateway(*resp),
    };
    let mut conn = ConnectionState {
        authenticated: true,
        token,
        principal_id: Some(principal_id),
        owned_sessions: Vec::new(),
    };

    let route_resp = handle_channel_inbound(
        ChannelInboundRequest {
            channel,
            account_id: &payload.account_id,
            external_user_id: &payload.external_user_id,
            text,
            create_response,
            session_config_on_create: Some(SessionConfig {
                model: None,
                voice: None,
                instructions: None,
                role: Some("supervised".to_string()),
                enable_graph: Some(true),
            }),
            force_session_recreate,
        },
        &state,
        &mut conn,
        None,
        None,
    )
    .await;

    let (channel, account_id, external_user_id, session_id) = match route_resp {
        GatewayResponse::ChannelRouted {
            channel,
            account_id,
            external_user_id,
            session_id,
        } => (channel, account_id, external_user_id, session_id),
        other => return http_response_from_gateway(other),
    };

    let (ws_tx, _ws_rx) = mpsc::channel(4);
    let (ws_priority_tx, _ws_priority_rx) = mpsc::channel(4);
    let tool_resp = handle_session_tool_call(
        &session_id,
        &payload.tool_name,
        payload.arguments,
        &state,
        &mut conn,
        &ws_tx,
        &ws_priority_tx,
    )
    .await;

    match tool_resp {
        GatewayResponse::SessionToolResult {
            tool_name,
            result,
            graph,
            ..
        } => (
            StatusCode::OK,
            Json(serde_json::json!({
                "type": "WebhookSupervisedActionResult",
                "channel": channel,
                "account_id": account_id,
                "external_user_id": external_user_id,
                "session_id": session_id,
                "tool_name": tool_name,
                "result": result,
                "graph": graph,
            })),
        ),
        other => http_response_from_gateway(other),
    }
}

fn extract_channel_token(headers: &HeaderMap, query: &ChannelIngressQuery) -> Option<String> {
    let from_header = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|raw| {
            raw.strip_prefix("Bearer ")
                .or_else(|| raw.strip_prefix("bearer "))
        })
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(ToOwned::to_owned);
    from_header.or_else(|| {
        query
            .token
            .as_ref()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
    })
}

async fn handle_channel_http_route(
    request: ChannelInboundRequest<'_>,
    token: Option<String>,
    state: &AppState,
) -> GatewayResponse {
    let principal_id = match authenticate_http_principal(token.as_deref(), state) {
        Ok(id) => id,
        Err(resp) => return *resp,
    };

    let mut conn = ConnectionState {
        authenticated: true,
        token,
        principal_id: Some(principal_id),
        owned_sessions: Vec::new(),
    };
    handle_channel_inbound(request, state, &mut conn, None, None).await
}

fn authenticate_http_principal(
    token: Option<&str>,
    state: &AppState,
) -> Result<String, Box<GatewayResponse>> {
    if state.pairing.is_authenticated("") {
        return Ok("anonymous".to_string());
    }

    let Some(token) = token else {
        return Err(Box::new(auth_required_error()));
    };
    if !state.pairing.is_authenticated(token) {
        return Err(Box::new(GatewayResponse::Error {
            code: "invalid_token".to_string(),
            message: "Token authentication failed".to_string(),
        }));
    }
    Ok(principal_from_token(token))
}

async fn handle_channel_http_outbound_poll(
    request: ChannelOutboundPollRequest<'_>,
    token: Option<&str>,
    state: &AppState,
) -> GatewayResponse {
    let principal_id = match authenticate_http_principal(token, state) {
        Ok(id) => id,
        Err(resp) => return *resp,
    };

    match drain_channel_outbox(
        state,
        &principal_id,
        request.channel,
        request.account_id,
        request.external_user_id,
        request.max_items,
    )
    .await
    {
        Ok((channel, account_id, external_user_id, items)) => {
            GatewayResponse::ChannelOutboundBatch {
                channel,
                account_id,
                external_user_id,
                items,
            }
        }
        Err(resp) => resp,
    }
}

async fn handle_channel_http_create_job(
    request: ChannelJobCreateRequest<'_>,
    token: Option<String>,
    state: &AppState,
) -> GatewayResponse {
    let principal_id = match authenticate_http_principal(token.as_deref(), state) {
        Ok(id) => id,
        Err(resp) => return *resp,
    };
    let conn = ConnectionState {
        authenticated: true,
        token,
        principal_id: Some(principal_id),
        owned_sessions: Vec::new(),
    };
    handle_create_channel_job(request, state, &conn).await
}

async fn handle_channel_http_cancel_job(
    job_id: &str,
    token: Option<String>,
    state: &AppState,
) -> GatewayResponse {
    let principal_id = match authenticate_http_principal(token.as_deref(), state) {
        Ok(id) => id,
        Err(resp) => return *resp,
    };
    let conn = ConnectionState {
        authenticated: true,
        token,
        principal_id: Some(principal_id),
        owned_sessions: Vec::new(),
    };
    handle_cancel_channel_job(job_id, state, &conn).await
}

async fn handle_channel_http_list_jobs(token: Option<String>, state: &AppState) -> GatewayResponse {
    let principal_id = match authenticate_http_principal(token.as_deref(), state) {
        Ok(id) => id,
        Err(resp) => return *resp,
    };
    let conn = ConnectionState {
        authenticated: true,
        token,
        principal_id: Some(principal_id),
        owned_sessions: Vec::new(),
    };
    handle_list_channel_jobs(state, &conn).await
}

fn http_response_from_gateway(resp: GatewayResponse) -> (StatusCode, Json<serde_json::Value>) {
    match resp {
        GatewayResponse::Error { code, message } => {
            let status = match code.as_str() {
                "auth_required" | "invalid_token" => StatusCode::UNAUTHORIZED,
                "invalid_channel" | "invalid_channel_route" | "invalid_channel_message" => {
                    StatusCode::BAD_REQUEST
                }
                "invalid_channel_job" => StatusCode::BAD_REQUEST,
                "forbidden_channel_job" => StatusCode::FORBIDDEN,
                "channel_job_not_found" => StatusCode::NOT_FOUND,
                _ => StatusCode::INTERNAL_SERVER_ERROR,
            };
            http_json_error(status, &code, &message)
        }
        GatewayResponse::ChannelRouted {
            channel,
            account_id,
            external_user_id,
            session_id,
        } => (
            StatusCode::OK,
            Json(serde_json::json!({
                "type": "ChannelRouted",
                "channel": channel,
                "account_id": account_id,
                "external_user_id": external_user_id,
                "session_id": session_id
            })),
        ),
        GatewayResponse::ChannelOutboundBatch {
            channel,
            account_id,
            external_user_id,
            items,
        } => (
            StatusCode::OK,
            Json(serde_json::json!({
                "type": "ChannelOutboundBatch",
                "channel": channel,
                "account_id": account_id,
                "external_user_id": external_user_id,
                "items": items,
            })),
        ),
        GatewayResponse::ChannelJobCreated { job } => (
            StatusCode::OK,
            Json(serde_json::json!({
                "type": "ChannelJobCreated",
                "job": job,
            })),
        ),
        GatewayResponse::ChannelJobCanceled { job_id } => (
            StatusCode::OK,
            Json(serde_json::json!({
                "type": "ChannelJobCanceled",
                "job_id": job_id,
            })),
        ),
        GatewayResponse::ChannelJobs { jobs } => (
            StatusCode::OK,
            Json(serde_json::json!({
                "type": "ChannelJobs",
                "jobs": jobs,
            })),
        ),
        other => (
            StatusCode::OK,
            Json(serde_json::json!({
                "type": "UnexpectedResponse",
                "payload": other,
            })),
        ),
    }
}

fn http_json_error(
    status: StatusCode,
    code: &str,
    message: &str,
) -> (StatusCode, Json<serde_json::Value>) {
    (
        status,
        Json(serde_json::json!({
            "type": "Error",
            "code": code,
            "message": message,
        })),
    )
}

// ---------------------------------------------------------------------------
// WebSocket connection handler
// ---------------------------------------------------------------------------

/// Per-connection state tracking authentication and owned sessions.
struct ConnectionState {
    authenticated: bool,
    token: Option<String>,
    /// Stable principal identity derived from pairing token hash. Used to
    /// enforce ownership across reconnects.
    principal_id: Option<String>,
    /// Session IDs owned by this connection, used to register/unregister
    /// ws_response_senders for audio/transcript forwarding.
    owned_sessions: Vec<String>,
}

impl ConnectionState {
    fn new() -> Self {
        Self {
            authenticated: false,
            token: None,
            principal_id: None,
            owned_sessions: Vec::new(),
        }
    }
}

/// Handle a single WebSocket connection.
///
/// Reads messages from the client, routes them through `handle_message`,
/// and sends responses back. Also listens for server-pushed responses
/// (AudioOutput, TranscriptUpdate) via a per-connection mpsc channel.
async fn handle_ws(mut ws: WebSocket, state: AppState) {
    let mut conn = ConnectionState::new();

    // If pairing is disabled, auto-authenticate
    if state.pairing.is_authenticated("") {
        conn.authenticated = true;
        conn.principal_id = Some("anonymous".to_string());
    }

    // Per-connection channels for server-pushed responses.
    let (ws_standard_tx, mut ws_standard_rx) = mpsc::channel::<GatewayResponse>(256);
    let (ws_priority_tx, mut ws_priority_rx) = mpsc::channel::<GatewayResponse>(64);

    loop {
        tokio::select! {
            biased;

            // Priority channel traffic is always processed before standard
            // channel traffic and incoming client frames.
            Some(resp) = ws_priority_rx.recv() => {
                if send_response(&mut ws, &resp).await.is_err() {
                    break;
                }
            }

            // Client → Server: incoming WebSocket messages
            msg_result = recv_message(&mut ws) => {
                let msg_result = match msg_result {
                    Some(r) => r,
                    None => break, // Stream ended
                };
                let text = match msg_result {
                    Ok(text) => text,
                    Err(()) => break, // Connection closed or error
                };

                let gateway_msg = match serde_json::from_str::<GatewayMessage>(&text) {
                    Ok(m) => m,
                    Err(e) => {
                        let resp = GatewayResponse::Error {
                            code: "invalid_message".to_string(),
                            message: format!("Failed to parse message: {}", e),
                        };
                        if send_response(&mut ws, &resp).await.is_err() {
                            break;
                        }
                        continue;
                    }
                };

                let resp = handle_message(
                    gateway_msg,
                    &state,
                    &mut conn,
                    &ws_standard_tx,
                    &ws_priority_tx,
                )
                .await;
                if send_response(&mut ws, &resp).await.is_err() {
                    break;
                }
            }

            // Server → Client: forwarded AudioOutput / TranscriptUpdate responses
            Some(resp) = ws_standard_rx.recv() => {
                if send_response(&mut ws, &resp).await.is_err() {
                    break;
                }
            }
        }
    }

    // Cleanup: remove ws_response_senders for all sessions owned by this connection
    {
        let mut senders = state.ws_response_senders.write().await;
        let mut priority_senders = state.ws_priority_senders.write().await;
        for session_id in &conn.owned_sessions {
            senders.remove(session_id);
            priority_senders.remove(session_id);
        }
    }

    info!("WebSocket connection closed");
}

// ---------------------------------------------------------------------------
// Message routing
// ---------------------------------------------------------------------------

/// Route a parsed GatewayMessage to the appropriate handler and return a response.
///
/// - `Pair` is always allowed (it's how clients authenticate).
/// - `Ping` is always allowed (keepalive).
/// - All session commands require authentication.
async fn handle_message(
    msg: GatewayMessage,
    state: &AppState,
    conn: &mut ConnectionState,
    ws_tx: &mpsc::Sender<GatewayResponse>,
    ws_priority_tx: &mpsc::Sender<GatewayResponse>,
) -> GatewayResponse {
    match msg {
        GatewayMessage::Ping => GatewayResponse::Pong,

        GatewayMessage::Pair { code } => handle_pair(&code, state, conn),

        GatewayMessage::Authenticate { token } => handle_authenticate(&token, state, conn),

        GatewayMessage::PriorityProbe => {
            if !conn.authenticated {
                return auth_required_error();
            }
            handle_priority_probe(conn, ws_tx, ws_priority_tx).await
        }

        GatewayMessage::GetGatewayHealth => handle_get_gateway_health(state).await,

        GatewayMessage::CreateSession { config } => {
            if !conn.authenticated {
                return auth_required_error();
            }
            handle_create_session(config, state, conn, ws_tx, ws_priority_tx).await
        }

        GatewayMessage::TerminateSession { session_id } => {
            if !conn.authenticated {
                return auth_required_error();
            }
            handle_terminate_session(&session_id, state, conn).await
        }

        GatewayMessage::SessionAudio { session_id, audio } => {
            if !conn.authenticated {
                return auth_required_error();
            }
            handle_session_audio(&session_id, &audio, state, conn, ws_tx, ws_priority_tx).await
        }

        GatewayMessage::SessionAudioCommit { session_id } => {
            if !conn.authenticated {
                return auth_required_error();
            }
            handle_session_audio_commit(&session_id, state, conn, ws_tx, ws_priority_tx).await
        }

        GatewayMessage::SessionResponseCreate { session_id } => {
            if !conn.authenticated {
                return auth_required_error();
            }
            handle_session_response_create(&session_id, state, conn, ws_tx, ws_priority_tx).await
        }

        GatewayMessage::SessionResponseInterrupt { session_id } => {
            if !conn.authenticated {
                return auth_required_error();
            }
            handle_session_response_interrupt(&session_id, state, conn, ws_tx, ws_priority_tx).await
        }

        GatewayMessage::SessionPrompt {
            session_id,
            prompt,
            create_response,
        } => {
            if !conn.authenticated {
                return auth_required_error();
            }
            handle_session_prompt(
                &session_id,
                &prompt,
                create_response.unwrap_or(true),
                state,
                conn,
                ws_tx,
                ws_priority_tx,
            )
            .await
        }

        GatewayMessage::ChannelInbound {
            channel,
            account_id,
            external_user_id,
            text,
            create_response,
        } => {
            if !conn.authenticated {
                return auth_required_error();
            }
            handle_channel_inbound(
                ChannelInboundRequest {
                    channel: &channel,
                    account_id: &account_id,
                    external_user_id: &external_user_id,
                    text: &text,
                    create_response: create_response.unwrap_or(true),
                    session_config_on_create: None,
                    force_session_recreate: false,
                },
                state,
                conn,
                Some(ws_tx),
                Some(ws_priority_tx),
            )
            .await
        }

        GatewayMessage::GetChannelOutbound {
            channel,
            account_id,
            external_user_id,
            max_items,
        } => {
            if !conn.authenticated {
                return auth_required_error();
            }
            handle_get_channel_outbound(
                &channel,
                &account_id,
                &external_user_id,
                max_items,
                state,
                conn,
            )
            .await
        }

        GatewayMessage::CreateChannelJob {
            channel,
            account_id,
            external_user_id,
            text,
            interval_seconds,
            create_response,
        } => {
            if !conn.authenticated {
                return auth_required_error();
            }
            handle_create_channel_job(
                ChannelJobCreateRequest {
                    channel: &channel,
                    account_id: &account_id,
                    external_user_id: &external_user_id,
                    text: &text,
                    interval_seconds,
                    create_response: create_response.unwrap_or(true),
                },
                state,
                conn,
            )
            .await
        }

        GatewayMessage::CancelChannelJob { job_id } => {
            if !conn.authenticated {
                return auth_required_error();
            }
            handle_cancel_channel_job(&job_id, state, conn).await
        }

        GatewayMessage::ListChannelJobs => {
            if !conn.authenticated {
                return auth_required_error();
            }
            handle_list_channel_jobs(state, conn).await
        }

        GatewayMessage::SessionToolCall {
            session_id,
            tool_name,
            arguments,
        } => {
            if !conn.authenticated {
                return auth_required_error();
            }
            handle_session_tool_call(
                &session_id,
                &tool_name,
                arguments,
                state,
                conn,
                ws_tx,
                ws_priority_tx,
            )
            .await
        }

        GatewayMessage::GetDiagnostics => {
            if !conn.authenticated {
                return auth_required_error();
            }
            handle_get_diagnostics(state).await
        }
    }
}

/// Construct the standard auth_required error response.
fn auth_required_error() -> GatewayResponse {
    GatewayResponse::Error {
        code: "auth_required".to_string(),
        message: "Authentication required. Send Pair or Authenticate first.".to_string(),
    }
}

// ---------------------------------------------------------------------------
// Individual message handlers
// ---------------------------------------------------------------------------

/// Handle a Pair message — delegates to PairingGuard.
fn handle_pair(code: &str, state: &AppState, conn: &mut ConnectionState) -> GatewayResponse {
    match state.pairing.try_pair(code) {
        Ok(Some(token)) => {
            let principal_id = principal_from_token(&token);
            conn.authenticated = true;
            conn.token = Some(token.clone());
            conn.principal_id = Some(principal_id);
            GatewayResponse::PairSuccess { token }
        }
        Ok(None) => {
            // Pairing disabled — auto-authenticated
            conn.authenticated = true;
            conn.principal_id = Some("anonymous".to_string());
            GatewayResponse::PairSuccess {
                token: "no-auth-required".to_string(),
            }
        }
        Err(0) => GatewayResponse::PairFailure {
            reason: "Invalid pairing code".to_string(),
        },
        Err(lockout_secs) => GatewayResponse::PairFailure {
            reason: format!("Locked out for {} seconds", lockout_secs),
        },
    }
}

/// Handle an Authenticate message using an existing token.
fn handle_authenticate(
    token: &str,
    state: &AppState,
    conn: &mut ConnectionState,
) -> GatewayResponse {
    if !state.pairing.is_authenticated(token) {
        return GatewayResponse::Error {
            code: "invalid_token".to_string(),
            message: "Token authentication failed".to_string(),
        };
    }

    let principal_id = if state.pairing.is_authenticated("") {
        "anonymous".to_string()
    } else {
        principal_from_token(token)
    };

    conn.authenticated = true;
    conn.token = Some(token.to_string());
    conn.principal_id = Some(principal_id.clone());

    GatewayResponse::Authenticated { principal_id }
}

/// Handle a CreateSession message — delegates to Runner and registers the
/// session's ws_response_sender so audio/transcript output can be forwarded.
async fn handle_create_session(
    config: Option<SessionConfig>,
    state: &AppState,
    conn: &mut ConnectionState,
    ws_tx: &mpsc::Sender<GatewayResponse>,
    ws_priority_tx: &mpsc::Sender<GatewayResponse>,
) -> GatewayResponse {
    handle_create_session_internal(config, state, conn, Some(ws_tx), Some(ws_priority_tx)).await
}

async fn handle_create_session_internal(
    config: Option<SessionConfig>,
    state: &AppState,
    conn: &mut ConnectionState,
    ws_tx: Option<&mpsc::Sender<GatewayResponse>>,
    ws_priority_tx: Option<&mpsc::Sender<GatewayResponse>>,
) -> GatewayResponse {
    let principal_id = conn
        .principal_id
        .clone()
        .unwrap_or_else(|| "unknown".to_string());
    let session_id = uuid::Uuid::new_v4().to_string();

    match state
        .runner
        .create_session(&principal_id, &session_id, config)
        .await
    {
        Ok(sid) => {
            state
                .session_owners
                .write()
                .await
                .insert(sid.clone(), principal_id);

            // Register the ws_response_sender so background tasks can forward
            // AudioOutput and TranscriptUpdate to this connection.
            if let Some(tx) = ws_tx {
                state
                    .ws_response_senders
                    .write()
                    .await
                    .insert(sid.clone(), tx.clone());
            }
            if let Some(tx) = ws_priority_tx {
                state
                    .ws_priority_senders
                    .write()
                    .await
                    .insert(sid.clone(), tx.clone());
            }
            if !conn.owned_sessions.contains(&sid) {
                conn.owned_sessions.push(sid.clone());
            }
            GatewayResponse::SessionCreated { session_id: sid }
        }
        Err(e) => {
            error!("Failed to create session: {}", e);
            GatewayResponse::Error {
                code: "session_create_failed".to_string(),
                message: format!("Failed to create session: {}", e),
            }
        }
    }
}

/// Handle a TerminateSession message — delegates to Runner and cleans up
/// audio sender and ws_response_sender.
async fn handle_terminate_session(
    session_id: &str,
    state: &AppState,
    conn: &mut ConnectionState,
) -> GatewayResponse {
    if let Err(resp) = authorize_session_access(session_id, state, conn, None, None).await {
        return resp;
    }

    // Remove audio sender for this session
    state.audio_senders.write().await.remove(session_id);
    // Remove ws_response_sender for this session
    state.ws_response_senders.write().await.remove(session_id);
    // Remove priority sender for this session
    state.ws_priority_senders.write().await.remove(session_id);
    // Remove stable session owner
    state.session_owners.write().await.remove(session_id);
    // Remove channel route bindings that reference this session.
    let removed_route_keys = {
        let mut routes = state.channel_routes.write().await;
        let mut removed = Vec::new();
        routes.retain(|route_key, mapped_session_id| {
            if mapped_session_id == session_id {
                removed.push(route_key.clone());
                false
            } else {
                true
            }
        });
        removed
    };
    state
        .session_channel_routes
        .write()
        .await
        .remove(session_id);
    if !removed_route_keys.is_empty() {
        let mut outbox = state.channel_outbox.write().await;
        for route_key in removed_route_keys {
            outbox.remove(&route_key);
        }
    }
    // Remove from owned sessions
    conn.owned_sessions.retain(|s| s != session_id);

    match state.runner.terminate_session(session_id).await {
        Ok(()) => GatewayResponse::SessionTerminated {
            session_id: session_id.to_string(),
        },
        Err(e) => {
            warn!("Failed to terminate session {}: {}", session_id, e);
            GatewayResponse::Error {
                code: "session_terminate_failed".to_string(),
                message: format!("Failed to terminate session: {}", e),
            }
        }
    }
}

/// Handle a SessionAudio message — decode base64 audio and forward to Runner.
async fn handle_session_audio(
    session_id: &str,
    audio_b64: &str,
    state: &AppState,
    conn: &mut ConnectionState,
    ws_tx: &mpsc::Sender<GatewayResponse>,
    ws_priority_tx: &mpsc::Sender<GatewayResponse>,
) -> GatewayResponse {
    if let Err(resp) =
        authorize_session_access(session_id, state, conn, Some(ws_tx), Some(ws_priority_tx)).await
    {
        return resp;
    }

    // Decode base64 audio data
    let audio_bytes = match base64_decode(audio_b64) {
        Some(bytes) => bytes,
        None => {
            return GatewayResponse::Error {
                code: "invalid_audio".to_string(),
                message: "Failed to decode base64 audio data".to_string(),
            };
        }
    };

    match state.runner.send_audio(session_id, &audio_bytes).await {
        Ok(()) => {
            // Audio forwarded successfully; acknowledge with protocol-specific
            // audio acceptance instead of reusing session creation semantics.
            GatewayResponse::AudioAccepted {
                session_id: session_id.to_string(),
            }
        }
        Err(e) => {
            warn!("Failed to send audio for session {}: {}", session_id, e);
            GatewayResponse::Error {
                code: "audio_send_failed".to_string(),
                message: format!("Failed to forward audio: {}", e),
            }
        }
    }
}

/// Handle SessionAudioCommit — commit the provider-side buffered input audio.
async fn handle_session_audio_commit(
    session_id: &str,
    state: &AppState,
    conn: &mut ConnectionState,
    ws_tx: &mpsc::Sender<GatewayResponse>,
    ws_priority_tx: &mpsc::Sender<GatewayResponse>,
) -> GatewayResponse {
    if let Err(resp) =
        authorize_session_access(session_id, state, conn, Some(ws_tx), Some(ws_priority_tx)).await
    {
        return resp;
    }

    match state.runner.commit_audio(session_id).await {
        Ok(()) => GatewayResponse::AudioCommitted {
            session_id: session_id.to_string(),
        },
        Err(e) => {
            warn!("Failed to commit audio for session {}: {}", session_id, e);
            GatewayResponse::Error {
                code: "audio_commit_failed".to_string(),
                message: format!("Failed to commit audio: {}", e),
            }
        }
    }
}

/// Handle SessionResponseCreate — request response generation from current input.
async fn handle_session_response_create(
    session_id: &str,
    state: &AppState,
    conn: &mut ConnectionState,
    ws_tx: &mpsc::Sender<GatewayResponse>,
    ws_priority_tx: &mpsc::Sender<GatewayResponse>,
) -> GatewayResponse {
    if let Err(resp) =
        authorize_session_access(session_id, state, conn, Some(ws_tx), Some(ws_priority_tx)).await
    {
        return resp;
    }

    match state.runner.create_response(session_id).await {
        Ok(()) => GatewayResponse::ResponseCreateAccepted {
            session_id: session_id.to_string(),
        },
        Err(e) => {
            warn!(
                "Failed to create response for session {}: {}",
                session_id, e
            );
            GatewayResponse::Error {
                code: "response_create_failed".to_string(),
                message: format!("Failed to create response: {}", e),
            }
        }
    }
}

/// Handle SessionResponseInterrupt — cancel current output generation.
async fn handle_session_response_interrupt(
    session_id: &str,
    state: &AppState,
    conn: &mut ConnectionState,
    ws_tx: &mpsc::Sender<GatewayResponse>,
    ws_priority_tx: &mpsc::Sender<GatewayResponse>,
) -> GatewayResponse {
    if let Err(resp) =
        authorize_session_access(session_id, state, conn, Some(ws_tx), Some(ws_priority_tx)).await
    {
        return resp;
    }

    match state.runner.interrupt_response(session_id).await {
        Ok(()) => GatewayResponse::ResponseInterruptAccepted {
            session_id: session_id.to_string(),
        },
        Err(e) => {
            warn!(
                "Failed to interrupt response for session {}: {}",
                session_id, e
            );
            GatewayResponse::Error {
                code: "response_interrupt_failed".to_string(),
                message: format!("Failed to interrupt response: {}", e),
            }
        }
    }
}

fn maybe_extract_workspace_read_path(prompt: &str) -> Option<String> {
    let lower = prompt.to_ascii_lowercase();
    let read_intent = lower.contains("read_workspace_file")
        || lower.contains("read file")
        || lower.contains("read the file")
        || lower.contains("readme");
    if !read_intent {
        return None;
    }

    for token in prompt.split_whitespace() {
        let candidate = token.trim_matches(|c: char| {
            matches!(
                c,
                '"' | '\'' | '`' | ',' | ';' | ':' | '(' | ')' | '[' | ']' | '{' | '}'
            )
        });
        if candidate.is_empty() {
            continue;
        }

        let normalized = candidate.trim_start_matches("./");
        let candidate_lower = normalized.to_ascii_lowercase();
        let is_file_like = candidate.contains('/') || candidate.contains('.');
        let known_ext = [
            ".md", ".txt", ".toml", ".json", ".yaml", ".yml", ".rs", ".ts", ".js", ".py", ".sh",
        ]
        .iter()
        .any(|ext| candidate_lower.ends_with(ext));
        if is_file_like && known_ext {
            return Some(normalized.to_string());
        }
    }

    if lower.contains("readme") {
        return Some("README.md".to_string());
    }

    None
}

fn build_workspace_summary_prompt(
    user_prompt: &str,
    path: &str,
    content: &str,
    truncated: bool,
) -> String {
    let truncation_note = if truncated {
        "The file content was truncated for safety. If needed, ask for a narrower section."
    } else {
        "The file content is complete for this read."
    };

    format!(
        "User request:\n{}\n\nContext from read_workspace_file(path=\"{}\"):\n{}\n\n{}\n\nNow answer the user request directly.",
        user_prompt, path, content, truncation_note
    )
}

fn normalize_channel(channel: &str) -> Option<String> {
    match channel.trim().to_ascii_lowercase().as_str() {
        "telegram" => Some("telegram".to_string()),
        "slack" => Some("slack".to_string()),
        "webhook" => Some("webhook".to_string()),
        _ => None,
    }
}

fn normalize_channel_component(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn bounded_max_items(value: Option<usize>) -> usize {
    value.unwrap_or(50).clamp(1, 500)
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

async fn drain_channel_outbox(
    state: &AppState,
    principal_id: &str,
    channel: &str,
    account_id: &str,
    external_user_id: &str,
    max_items: Option<usize>,
) -> Result<(String, String, String, Vec<ChannelOutboundItem>), GatewayResponse> {
    let Some(channel) = normalize_channel(channel) else {
        return Err(GatewayResponse::Error {
            code: "invalid_channel".to_string(),
            message: "Supported channels are telegram, slack, and webhook".to_string(),
        });
    };
    let Some(account_id) = normalize_channel_component(account_id) else {
        return Err(GatewayResponse::Error {
            code: "invalid_channel_route".to_string(),
            message: "account_id is required".to_string(),
        });
    };
    let Some(external_user_id) = normalize_channel_component(external_user_id) else {
        return Err(GatewayResponse::Error {
            code: "invalid_channel_route".to_string(),
            message: "external_user_id is required".to_string(),
        });
    };
    let route_key = ChannelRouteKey {
        principal_id: principal_id.to_string(),
        channel: channel.clone(),
        account_id: account_id.clone(),
        external_user_id: external_user_id.clone(),
    };
    let take_n = bounded_max_items(max_items);
    let items = {
        let mut outbox = state.channel_outbox.write().await;
        if let Some(queue) = outbox.get_mut(&route_key) {
            let split = take_n.min(queue.len());
            let drained: Vec<ChannelOutboundItem> = queue.drain(0..split).collect();
            if queue.is_empty() {
                outbox.remove(&route_key);
            }
            drained
        } else {
            Vec::new()
        }
    };
    Ok((channel, account_id, external_user_id, items))
}

async fn handle_get_channel_outbound(
    channel: &str,
    account_id: &str,
    external_user_id: &str,
    max_items: Option<usize>,
    state: &AppState,
    conn: &ConnectionState,
) -> GatewayResponse {
    let Some(principal_id) = conn.principal_id.as_deref() else {
        return auth_required_error();
    };
    match drain_channel_outbox(
        state,
        principal_id,
        channel,
        account_id,
        external_user_id,
        max_items,
    )
    .await
    {
        Ok((channel, account_id, external_user_id, items)) => {
            GatewayResponse::ChannelOutboundBatch {
                channel,
                account_id,
                external_user_id,
                items,
            }
        }
        Err(resp) => resp,
    }
}

async fn handle_create_channel_job(
    request: ChannelJobCreateRequest<'_>,
    state: &AppState,
    conn: &ConnectionState,
) -> GatewayResponse {
    let Some(owner_principal_id) = conn.principal_id.clone() else {
        return auth_required_error();
    };

    let Some(channel) = normalize_channel(request.channel) else {
        return GatewayResponse::Error {
            code: "invalid_channel".to_string(),
            message: "Supported channels are telegram, slack, and webhook".to_string(),
        };
    };
    let Some(account_id) = normalize_channel_component(request.account_id) else {
        return GatewayResponse::Error {
            code: "invalid_channel_route".to_string(),
            message: "account_id is required".to_string(),
        };
    };
    let Some(external_user_id) = normalize_channel_component(request.external_user_id) else {
        return GatewayResponse::Error {
            code: "invalid_channel_route".to_string(),
            message: "external_user_id is required".to_string(),
        };
    };
    let Some(text) = normalize_channel_component(request.text) else {
        return GatewayResponse::Error {
            code: "invalid_channel_message".to_string(),
            message: "text is empty".to_string(),
        };
    };
    if request.interval_seconds == 0 {
        return GatewayResponse::Error {
            code: "invalid_channel_job".to_string(),
            message: "interval_seconds must be greater than 0".to_string(),
        };
    }

    let job = ChannelJob {
        job_id: uuid::Uuid::new_v4().to_string(),
        channel,
        account_id,
        external_user_id,
        text,
        interval_seconds: request.interval_seconds,
        create_response: request.create_response,
        created_at_unix_ms: now_unix_ms(),
    };

    let (cancel_tx, cancel_rx) = oneshot::channel();
    state.channel_jobs.write().await.insert(
        job.job_id.clone(),
        ChannelJobRuntime {
            owner_principal_id: owner_principal_id.clone(),
            job: job.clone(),
            cancel_tx: Some(cancel_tx),
        },
    );

    let job_state = state.clone();
    let job_config = job.clone();
    tokio::spawn(async move {
        run_channel_job(job_state, job_config, owner_principal_id, cancel_rx).await;
    });

    GatewayResponse::ChannelJobCreated { job }
}

async fn run_channel_job(
    state: AppState,
    job: ChannelJob,
    owner_principal_id: String,
    mut cancel_rx: oneshot::Receiver<()>,
) {
    info!(
        "Channel job {} started for {}/{}/{} every {}s",
        job.job_id, job.channel, job.account_id, job.external_user_id, job.interval_seconds
    );

    let mut ticker = time::interval(Duration::from_secs(job.interval_seconds));
    ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
    // First tick fires immediately by default; consume it so periodic jobs
    // start after one full interval.
    ticker.tick().await;

    loop {
        tokio::select! {
            _ = &mut cancel_rx => {
                break;
            }
            _ = ticker.tick() => {
                run_channel_job_tick(&state, &job, &owner_principal_id).await;
            }
        }
    }

    state.channel_jobs.write().await.remove(&job.job_id);
    info!("Channel job {} stopped", job.job_id);
}

async fn run_channel_job_tick(state: &AppState, job: &ChannelJob, owner_principal_id: &str) {
    let mut conn = ConnectionState {
        authenticated: true,
        token: None,
        principal_id: Some(owner_principal_id.to_string()),
        owned_sessions: Vec::new(),
    };
    let response = handle_channel_inbound(
        ChannelInboundRequest {
            channel: &job.channel,
            account_id: &job.account_id,
            external_user_id: &job.external_user_id,
            text: &job.text,
            create_response: job.create_response,
            session_config_on_create: None,
            force_session_recreate: false,
        },
        state,
        &mut conn,
        None,
        None,
    )
    .await;
    if let GatewayResponse::Error { code, message } = response {
        warn!(
            "Channel job {} tick failed with code='{}': {}",
            job.job_id, code, message
        );
    }
}

async fn handle_cancel_channel_job(
    job_id: &str,
    state: &AppState,
    conn: &ConnectionState,
) -> GatewayResponse {
    let Some(principal_id) = conn.principal_id.as_deref() else {
        return auth_required_error();
    };

    let runtime = {
        let mut jobs = state.channel_jobs.write().await;
        let Some(existing) = jobs.get(job_id) else {
            return GatewayResponse::Error {
                code: "channel_job_not_found".to_string(),
                message: format!("Channel job '{}' not found", job_id),
            };
        };
        if existing.owner_principal_id != principal_id {
            return GatewayResponse::Error {
                code: "forbidden_channel_job".to_string(),
                message: "Channel job is not owned by the authenticated principal".to_string(),
            };
        }
        jobs.remove(job_id)
    };

    if let Some(mut runtime) = runtime {
        if let Some(cancel_tx) = runtime.cancel_tx.take() {
            let _ = cancel_tx.send(());
        }
        GatewayResponse::ChannelJobCanceled {
            job_id: job_id.to_string(),
        }
    } else {
        GatewayResponse::Error {
            code: "channel_job_not_found".to_string(),
            message: format!("Channel job '{}' not found", job_id),
        }
    }
}

async fn handle_list_channel_jobs(state: &AppState, conn: &ConnectionState) -> GatewayResponse {
    let Some(principal_id) = conn.principal_id.as_deref() else {
        return auth_required_error();
    };

    let mut jobs: Vec<ChannelJob> = state
        .channel_jobs
        .read()
        .await
        .values()
        .filter(|runtime| runtime.owner_principal_id == principal_id)
        .map(|runtime| runtime.job.clone())
        .collect();
    jobs.sort_by(|a, b| {
        a.created_at_unix_ms
            .cmp(&b.created_at_unix_ms)
            .then_with(|| a.job_id.cmp(&b.job_id))
    });
    GatewayResponse::ChannelJobs { jobs }
}

async fn queue_channel_outbound_transcript(state: &AppState, output: &SessionTranscriptOutput) {
    if !output.is_final {
        return;
    }
    let text = output.text.trim();
    if text.is_empty() {
        return;
    }

    let route_key = {
        let routes = state.session_channel_routes.read().await;
        routes.get(&output.session_id).cloned()
    };
    let Some(route_key) = route_key else {
        return;
    };

    let item = ChannelOutboundItem {
        id: uuid::Uuid::new_v4().to_string(),
        session_id: output.session_id.clone(),
        text: text.to_string(),
        created_at_unix_ms: now_unix_ms(),
    };
    state
        .channel_outbox
        .write()
        .await
        .entry(route_key)
        .or_default()
        .push(item);
}

/// Handle SessionPrompt — forward a text prompt to the active provider-side
/// conversation, optionally triggering response generation.
async fn handle_session_prompt(
    session_id: &str,
    prompt: &str,
    create_response: bool,
    state: &AppState,
    conn: &mut ConnectionState,
    ws_tx: &mpsc::Sender<GatewayResponse>,
    ws_priority_tx: &mpsc::Sender<GatewayResponse>,
) -> GatewayResponse {
    if let Err(resp) =
        authorize_session_access(session_id, state, conn, Some(ws_tx), Some(ws_priority_tx)).await
    {
        return resp;
    }

    let trimmed = prompt.trim();
    if trimmed.is_empty() {
        return GatewayResponse::Error {
            code: "invalid_prompt".to_string(),
            message: "Prompt is empty".to_string(),
        };
    }

    let mut outbound_prompt = trimmed.to_string();
    if let Some(path) = maybe_extract_workspace_read_path(trimmed) {
        let tool_args = serde_json::json!({
            "path": path,
            "max_bytes": 32768,
        });
        match state
            .runner
            .session_tool_call(session_id, "read_workspace_file", tool_args)
            .await
        {
            Ok((tool_result, graph)) => {
                // Surface prompt-driven tool execution to the client for
                // operator/user validation.
                let _ = ws_tx
                    .send(GatewayResponse::SessionToolResult {
                        session_id: session_id.to_string(),
                        tool_name: "read_workspace_file".to_string(),
                        result: tool_result.clone(),
                        graph,
                    })
                    .await;

                let content = tool_result
                    .get("result")
                    .and_then(|v| v.get("content"))
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default();
                let resolved_path = tool_result
                    .get("result")
                    .and_then(|v| v.get("path"))
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("README.md");
                let truncated = tool_result
                    .get("result")
                    .and_then(|v| v.get("truncated"))
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false);
                if !content.is_empty() {
                    outbound_prompt =
                        build_workspace_summary_prompt(trimmed, resolved_path, content, truncated);
                }
            }
            Err(e) => {
                warn!(
                    "Prompt tool bridge failed for session {}: {}",
                    session_id, e
                );
                return GatewayResponse::Error {
                    code: "session_prompt_tool_bridge_failed".to_string(),
                    message: format!(
                        "Failed to execute prompt-driven read_workspace_file call: {}",
                        e
                    ),
                };
            }
        }
    }

    if let Err(e) = state.runner.send_text(session_id, &outbound_prompt).await {
        warn!("Failed to send prompt for session {}: {}", session_id, e);
        return GatewayResponse::Error {
            code: "session_prompt_failed".to_string(),
            message: format!("Failed to send prompt: {}", e),
        };
    }

    if create_response {
        if let Err(e) = state.runner.create_response(session_id).await {
            warn!(
                "Prompt sent but failed to create response for session {}: {}",
                session_id, e
            );
            return GatewayResponse::Error {
                code: "session_prompt_response_failed".to_string(),
                message: format!("Prompt queued, but create_response failed: {}", e),
            };
        }
    }

    GatewayResponse::PromptAccepted {
        session_id: session_id.to_string(),
    }
}

/// Handle ChannelInbound — map external channel/account/user identity to an
/// isolated session scoped to authenticated principal and forward text input.
async fn handle_channel_inbound(
    request: ChannelInboundRequest<'_>,
    state: &AppState,
    conn: &mut ConnectionState,
    ws_tx: Option<&mpsc::Sender<GatewayResponse>>,
    ws_priority_tx: Option<&mpsc::Sender<GatewayResponse>>,
) -> GatewayResponse {
    let Some(principal_id) = conn.principal_id.clone() else {
        return auth_required_error();
    };

    let Some(channel) = normalize_channel(request.channel) else {
        return GatewayResponse::Error {
            code: "invalid_channel".to_string(),
            message: "Supported channels are telegram, slack, and webhook".to_string(),
        };
    };
    let Some(account_id) = normalize_channel_component(request.account_id) else {
        return GatewayResponse::Error {
            code: "invalid_channel_route".to_string(),
            message: "account_id is required".to_string(),
        };
    };
    let Some(external_user_id) = normalize_channel_component(request.external_user_id) else {
        return GatewayResponse::Error {
            code: "invalid_channel_route".to_string(),
            message: "external_user_id is required".to_string(),
        };
    };
    let Some(text) = normalize_channel_component(request.text) else {
        return GatewayResponse::Error {
            code: "invalid_channel_message".to_string(),
            message: "text is empty".to_string(),
        };
    };

    let route_key = ChannelRouteKey {
        principal_id,
        channel: channel.clone(),
        account_id: account_id.clone(),
        external_user_id: external_user_id.clone(),
    };

    let mapped_session = {
        let routes = state.channel_routes.read().await;
        routes.get(&route_key).cloned()
    };
    let create_session_config = request.session_config_on_create.clone();

    if request.force_session_recreate {
        if let Some(existing_session_id) = mapped_session.as_ref() {
            state.channel_routes.write().await.remove(&route_key);
            state
                .session_channel_routes
                .write()
                .await
                .remove(existing_session_id);
            state.channel_outbox.write().await.remove(&route_key);
            state
                .ws_response_senders
                .write()
                .await
                .remove(existing_session_id);
            state
                .ws_priority_senders
                .write()
                .await
                .remove(existing_session_id);
            state
                .audio_senders
                .write()
                .await
                .remove(existing_session_id);
            state
                .session_owners
                .write()
                .await
                .remove(existing_session_id);
            if let Err(e) = state.runner.terminate_session(existing_session_id).await {
                warn!(
                    "Failed to terminate replaced channel session {}: {}",
                    existing_session_id, e
                );
            }
        }
    }
    let mapped_session = if request.force_session_recreate {
        None
    } else {
        mapped_session
    };

    let session_id = if let Some(existing_session_id) = mapped_session {
        let exists = state
            .session_owners
            .read()
            .await
            .contains_key(&existing_session_id);
        if exists {
            existing_session_id
        } else {
            state.channel_routes.write().await.remove(&route_key);
            state
                .session_channel_routes
                .write()
                .await
                .remove(&existing_session_id);
            let created = match handle_create_session_internal(
                create_session_config.clone(),
                state,
                conn,
                ws_tx,
                ws_priority_tx,
            )
            .await
            {
                GatewayResponse::SessionCreated { session_id } => session_id,
                other => return other,
            };
            state
                .channel_routes
                .write()
                .await
                .insert(route_key.clone(), created.clone());
            created
        }
    } else {
        let created = match handle_create_session_internal(
            create_session_config,
            state,
            conn,
            ws_tx,
            ws_priority_tx,
        )
        .await
        {
            GatewayResponse::SessionCreated { session_id } => session_id,
            other => return other,
        };
        state
            .channel_routes
            .write()
            .await
            .insert(route_key.clone(), created.clone());
        created
    };

    state
        .session_channel_routes
        .write()
        .await
        .insert(session_id.clone(), route_key);

    if let Err(resp) =
        authorize_session_access(&session_id, state, conn, ws_tx, ws_priority_tx).await
    {
        return resp;
    }

    if let Err(e) = state.runner.send_text(&session_id, &text).await {
        warn!(
            "Failed to forward channel '{}' message for session {}: {}",
            channel, session_id, e
        );
        return GatewayResponse::Error {
            code: "channel_inbound_failed".to_string(),
            message: format!("Failed to forward channel message: {}", e),
        };
    }

    if request.create_response {
        if let Err(e) = state.runner.create_response(&session_id).await {
            warn!(
                "Channel message forwarded but create_response failed for session {}: {}",
                session_id, e
            );
            return GatewayResponse::Error {
                code: "channel_response_failed".to_string(),
                message: format!(
                    "Channel message routed, but response creation failed: {}",
                    e
                ),
            };
        }
    }

    GatewayResponse::ChannelRouted {
        channel,
        account_id,
        external_user_id,
        session_id,
    }
}

/// Handle SessionToolCall — execute deterministic tool invocation and return
/// graph execution report for operator validation.
async fn handle_session_tool_call(
    session_id: &str,
    tool_name: &str,
    arguments: serde_json::Value,
    state: &AppState,
    conn: &mut ConnectionState,
    ws_tx: &mpsc::Sender<GatewayResponse>,
    ws_priority_tx: &mpsc::Sender<GatewayResponse>,
) -> GatewayResponse {
    if let Err(resp) =
        authorize_session_access(session_id, state, conn, Some(ws_tx), Some(ws_priority_tx)).await
    {
        return resp;
    }

    match state
        .runner
        .session_tool_call(session_id, tool_name, arguments)
        .await
    {
        Ok((result, graph)) => GatewayResponse::SessionToolResult {
            session_id: session_id.to_string(),
            tool_name: tool_name.to_string(),
            result,
            graph,
        },
        Err(e) => {
            warn!(
                "Failed to execute SessionToolCall '{}' for session {}: {}",
                tool_name, session_id, e
            );
            GatewayResponse::Error {
                code: "session_tool_call_failed".to_string(),
                message: format!("Failed to execute tool call: {}", e),
            }
        }
    }
}

/// Handle GetDiagnostics — returns runtime/provider/resilience/compaction
/// metrics from the active runner.
async fn handle_get_diagnostics(state: &AppState) -> GatewayResponse {
    match state.runner.diagnostics().await {
        Ok(data) => GatewayResponse::Diagnostics {
            data: Box::new(data),
        },
        Err(e) => GatewayResponse::Error {
            code: "diagnostics_failed".to_string(),
            message: format!("Failed to fetch diagnostics: {}", e),
        },
    }
}

/// Handle GetGatewayHealth — returns operational gateway metrics without
/// requiring session auth.
async fn handle_get_gateway_health(state: &AppState) -> GatewayResponse {
    let active_sessions = state.session_owners.read().await.len();
    let active_ws_bindings = state.ws_response_senders.read().await.len();
    let active_priority_bindings = state.ws_priority_senders.read().await.len();
    let active_channel_jobs = state.channel_jobs.read().await.len();
    let require_pairing = !state.pairing.is_authenticated("");
    GatewayResponse::GatewayHealth {
        data: GatewayHealth {
            protocol_version: PROTOCOL_VERSION.to_string(),
            uptime_seconds: state.started_at.elapsed().as_secs(),
            require_pairing,
            active_sessions,
            active_ws_bindings,
            active_priority_bindings,
            active_channel_jobs,
        },
    }
}

/// Handle PriorityProbe — enqueues one standard-plane event and one
/// priority-plane notice so operators can validate channel precedence.
async fn handle_priority_probe(
    conn: &ConnectionState,
    ws_tx: &mpsc::Sender<GatewayResponse>,
    ws_priority_tx: &mpsc::Sender<GatewayResponse>,
) -> GatewayResponse {
    let session_id = conn
        .owned_sessions
        .last()
        .cloned()
        .unwrap_or_else(|| "priority-probe".to_string());

    let queued_standard = ws_tx
        .send(GatewayResponse::TranscriptUpdate {
            session_id: session_id.clone(),
            text: "priority_probe_standard".to_string(),
            is_final: false,
        })
        .await
        .is_ok();

    let queued_priority = ws_priority_tx
        .send(GatewayResponse::PriorityNotice {
            data: PriorityNotice {
                level: "info".to_string(),
                code: "priority_probe".to_string(),
                message: "Priority channel is active".to_string(),
                session_id: Some(session_id),
            },
        })
        .await
        .is_ok();

    GatewayResponse::PriorityProbeAccepted {
        queued_standard,
        queued_priority,
    }
}

/// Compute a stable principal identity from a raw token.
fn principal_from_token(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Ensure the authenticated connection can operate on `session_id`.
///
/// Access is granted only when the connection principal matches the stable
/// session owner. On successful reconnect access, optionally rebinds the
/// session's ws_response_sender to this connection so pushed outputs resume.
async fn authorize_session_access(
    session_id: &str,
    state: &AppState,
    conn: &mut ConnectionState,
    ws_tx: Option<&mpsc::Sender<GatewayResponse>>,
    ws_priority_tx: Option<&mpsc::Sender<GatewayResponse>>,
) -> Result<(), GatewayResponse> {
    let Some(principal_id) = conn.principal_id.as_deref() else {
        return Err(auth_required_error());
    };

    if conn.owned_sessions.iter().any(|sid| sid == session_id) {
        if let Some(sender) = ws_tx {
            state
                .ws_response_senders
                .write()
                .await
                .insert(session_id.to_string(), sender.clone());
        }
        if let Some(sender) = ws_priority_tx {
            state
                .ws_priority_senders
                .write()
                .await
                .insert(session_id.to_string(), sender.clone());
        }
        return Ok(());
    }

    let owner = {
        let owners = state.session_owners.read().await;
        owners.get(session_id).cloned()
    };

    match owner {
        Some(owner_id) if owner_id == principal_id => {
            conn.owned_sessions.push(session_id.to_string());
            if let Some(sender) = ws_tx {
                state
                    .ws_response_senders
                    .write()
                    .await
                    .insert(session_id.to_string(), sender.clone());
            }
            if let Some(sender) = ws_priority_tx {
                state
                    .ws_priority_senders
                    .write()
                    .await
                    .insert(session_id.to_string(), sender.clone());
            }
            Ok(())
        }
        Some(_) => Err(GatewayResponse::Error {
            code: "forbidden_session".to_string(),
            message: "Session is not owned by the authenticated principal".to_string(),
        }),
        None => Err(GatewayResponse::Error {
            code: "session_not_found".to_string(),
            message: format!("Session '{}' not found", session_id),
        }),
    }
}

// ---------------------------------------------------------------------------
// WebSocket helpers
// ---------------------------------------------------------------------------

/// Receive the next text message from the WebSocket.
///
/// Returns `Some(Ok(text))` for a text frame, `Some(Err(()))` for close/error,
/// and `None` when the stream ends.
async fn recv_message(ws: &mut WebSocket) -> Option<Result<String, ()>> {
    loop {
        match ws.recv().await {
            Some(Ok(Message::Text(text))) => return Some(Ok(text.to_string())),
            Some(Ok(Message::Close(_))) => return Some(Err(())),
            Some(Ok(Message::Ping(_))) => {
                // axum auto-responds to pings, just continue
                continue;
            }
            Some(Ok(_)) => {
                // Skip binary and other frame types
                continue;
            }
            Some(Err(e)) => {
                warn!("WebSocket receive error: {}", e);
                return Some(Err(()));
            }
            None => return None,
        }
    }
}

/// Send a GatewayResponse as a JSON text frame.
async fn send_response(ws: &mut WebSocket, resp: &GatewayResponse) -> Result<(), ()> {
    let json = serde_json::to_string(resp).map_err(|_| ())?;
    ws.send(Message::Text(json.into())).await.map_err(|e| {
        warn!("WebSocket send error: {}", e);
    })
}

// ---------------------------------------------------------------------------
// Base64 helpers (minimal, avoids adding a dependency)
// ---------------------------------------------------------------------------

/// Decode a base64-encoded string to bytes.
/// Returns `None` if the input is not valid base64.
fn base64_decode(input: &str) -> Option<Vec<u8>> {
    fn val(c: u8) -> Option<u8> {
        match c {
            b'A'..=b'Z' => Some(c - b'A'),
            b'a'..=b'z' => Some(c - b'a' + 26),
            b'0'..=b'9' => Some(c - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }

    let input = input.trim_end_matches('=');
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len() * 3 / 4);

    for chunk in bytes.chunks(4) {
        let mut buf: u32 = 0;
        let mut count = 0;
        for &b in chunk {
            buf = (buf << 6) | val(b)? as u32;
            count += 1;
        }
        // Pad remaining bits
        buf <<= (4 - count) * 6;

        if count >= 2 {
            out.push((buf >> 16) as u8);
        }
        if count >= 3 {
            out.push((buf >> 8) as u8);
        }
        if count >= 4 {
            out.push(buf as u8);
        }
    }

    Some(out)
}

/// Encode bytes to a base64 string.
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pairing::PairingGuard;
    use crate::protocol::SessionConfig;
    use axum::body::{to_bytes, Body};
    use axum::http::{header, Request, StatusCode};
    use tower::ServiceExt;

    /// A mock RunnerHandle for testing message routing logic.
    struct MockRunner {
        /// If set, create_session returns this error.
        create_err: Option<String>,
        /// If set, terminate_session returns this error.
        terminate_err: Option<String>,
        /// If set, send_audio returns this error.
        audio_err: Option<String>,
        /// If set, commit_audio returns this error.
        audio_commit_err: Option<String>,
        /// If set, create_response returns this error.
        response_create_err: Option<String>,
        /// Count of create_response calls.
        response_create_calls: Arc<std::sync::Mutex<usize>>,
        /// If set, interrupt_response returns this error.
        response_interrupt_err: Option<String>,
        /// If set, send_text returns this error.
        send_text_err: Option<String>,
        /// If set, session_tool_call returns this error.
        tool_call_err: Option<String>,
        /// If set, diagnostics returns this error.
        diagnostics_err: Option<String>,
        /// Last prompt forwarded to send_text.
        last_prompt: Arc<std::sync::Mutex<Option<String>>>,
        /// Recorded tool calls: (tool_name, arguments).
        tool_calls: Arc<std::sync::Mutex<Vec<(String, serde_json::Value)>>>,
        /// Recorded configs passed into create_session.
        created_session_configs: Arc<std::sync::Mutex<Vec<Option<SessionConfig>>>>,
    }

    impl MockRunner {
        fn ok() -> Self {
            Self {
                create_err: None,
                terminate_err: None,
                audio_err: None,
                audio_commit_err: None,
                response_create_err: None,
                response_create_calls: Arc::new(std::sync::Mutex::new(0)),
                response_interrupt_err: None,
                send_text_err: None,
                tool_call_err: None,
                diagnostics_err: None,
                last_prompt: Arc::new(std::sync::Mutex::new(None)),
                tool_calls: Arc::new(std::sync::Mutex::new(Vec::new())),
                created_session_configs: Arc::new(std::sync::Mutex::new(Vec::new())),
            }
        }

        fn with_create_err(msg: &str) -> Self {
            Self {
                create_err: Some(msg.to_string()),
                terminate_err: None,
                audio_err: None,
                audio_commit_err: None,
                response_create_err: None,
                response_create_calls: Arc::new(std::sync::Mutex::new(0)),
                response_interrupt_err: None,
                send_text_err: None,
                tool_call_err: None,
                diagnostics_err: None,
                last_prompt: Arc::new(std::sync::Mutex::new(None)),
                tool_calls: Arc::new(std::sync::Mutex::new(Vec::new())),
                created_session_configs: Arc::new(std::sync::Mutex::new(Vec::new())),
            }
        }
    }

    #[async_trait]
    impl RunnerHandle for MockRunner {
        async fn create_session(
            &self,
            _user_id: &str,
            session_id: &str,
            config: Option<SessionConfig>,
        ) -> anyhow::Result<String> {
            self.created_session_configs
                .lock()
                .expect("created_session_configs mutex poisoned")
                .push(config);
            match &self.create_err {
                Some(e) => Err(anyhow::anyhow!("{}", e)),
                None => Ok(session_id.to_string()),
            }
        }

        async fn terminate_session(&self, _session_id: &str) -> anyhow::Result<()> {
            match &self.terminate_err {
                Some(e) => Err(anyhow::anyhow!("{}", e)),
                None => Ok(()),
            }
        }

        async fn send_audio(&self, _session_id: &str, _audio: &[u8]) -> anyhow::Result<()> {
            match &self.audio_err {
                Some(e) => Err(anyhow::anyhow!("{}", e)),
                None => Ok(()),
            }
        }

        async fn commit_audio(&self, _session_id: &str) -> anyhow::Result<()> {
            match &self.audio_commit_err {
                Some(e) => Err(anyhow::anyhow!("{}", e)),
                None => Ok(()),
            }
        }

        async fn create_response(&self, _session_id: &str) -> anyhow::Result<()> {
            let mut calls = self
                .response_create_calls
                .lock()
                .expect("response_create_calls mutex poisoned");
            *calls += 1;
            match &self.response_create_err {
                Some(e) => Err(anyhow::anyhow!("{}", e)),
                None => Ok(()),
            }
        }

        async fn interrupt_response(&self, _session_id: &str) -> anyhow::Result<()> {
            match &self.response_interrupt_err {
                Some(e) => Err(anyhow::anyhow!("{}", e)),
                None => Ok(()),
            }
        }

        async fn send_text(&self, _session_id: &str, prompt: &str) -> anyhow::Result<()> {
            *self.last_prompt.lock().expect("last_prompt mutex poisoned") =
                Some(prompt.to_string());
            match &self.send_text_err {
                Some(e) => Err(anyhow::anyhow!("{}", e)),
                None => Ok(()),
            }
        }

        async fn session_tool_call(
            &self,
            _session_id: &str,
            tool_name: &str,
            arguments: serde_json::Value,
        ) -> anyhow::Result<(serde_json::Value, GraphExecutionReport)> {
            self.tool_calls
                .lock()
                .expect("tool_calls mutex poisoned")
                .push((tool_name.to_string(), arguments.clone()));
            match &self.tool_call_err {
                Some(e) => Err(anyhow::anyhow!("{}", e)),
                None => Ok((
                    serde_json::json!({
                        "tool": tool_name,
                        "status": "ok",
                        "result": {
                            "path": arguments
                                .get("path")
                                .and_then(serde_json::Value::as_str)
                                .unwrap_or("README.md"),
                            "content": "LiveClaw README synthetic content",
                            "truncated": false
                        }
                    }),
                    GraphExecutionReport {
                        thread_id: "test-thread".to_string(),
                        completed: true,
                        interrupted: false,
                        events: vec![crate::protocol::GraphExecutionEvent {
                            step: 0,
                            node: "tools".to_string(),
                            action: "node_end".to_string(),
                            detail: "tool executed".to_string(),
                        }],
                        final_state: serde_json::json!({
                            "tools_executed_count": 1
                        }),
                    },
                )),
            }
        }

        async fn diagnostics(&self) -> anyhow::Result<RuntimeDiagnostics> {
            match &self.diagnostics_err {
                Some(e) => Err(anyhow::anyhow!("{}", e)),
                None => Ok(RuntimeDiagnostics {
                    protocol_version: crate::protocol::PROTOCOL_VERSION.to_string(),
                    supported_client_messages: crate::protocol::supported_client_message_types(),
                    supported_server_responses: crate::protocol::supported_server_response_types(),
                    runtime_kind: "native".to_string(),
                    provider_profile: "legacy".to_string(),
                    provider_kind: "openai".to_string(),
                    provider_model: "gpt-4o-realtime-preview".to_string(),
                    provider_base_url: Some("wss://api.openai.com/v1/realtime".to_string()),
                    reconnect_enabled: true,
                    reconnect_max_attempts: 5,
                    reconnect_attempts_total: 2,
                    reconnect_successes_total: 1,
                    reconnect_failures_total: 1,
                    compaction_enabled: true,
                    compaction_max_events_threshold: 500,
                    compactions_applied_total: 3,
                    security_workspace_root: ".".to_string(),
                    security_forbidden_tool_paths: vec![".git".to_string(), "target".to_string()],
                    security_deny_by_default_principal_allowlist: false,
                    security_principal_allowlist_size: 0,
                    security_allow_public_bind: false,
                    active_sessions: 1,
                }),
            }
        }
    }

    fn make_state(runner: MockRunner, require_pairing: bool) -> AppState {
        AppState {
            pairing: Arc::new(PairingGuard::new(require_pairing, &[])),
            runner: Arc::new(runner),
            started_at: Instant::now(),
            audio_senders: Arc::new(RwLock::new(HashMap::new())),
            session_owners: Arc::new(RwLock::new(HashMap::new())),
            ws_response_senders: Arc::new(RwLock::new(HashMap::new())),
            ws_priority_senders: Arc::new(RwLock::new(HashMap::new())),
            channel_routes: Arc::new(RwLock::new(HashMap::new())),
            session_channel_routes: Arc::new(RwLock::new(HashMap::new())),
            channel_outbox: Arc::new(RwLock::new(HashMap::new())),
            channel_jobs: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    fn authed_conn() -> ConnectionState {
        ConnectionState {
            authenticated: true,
            token: Some("test-token".to_string()),
            principal_id: Some("user-1".to_string()),
            owned_sessions: Vec::new(),
        }
    }

    fn unauthed_conn() -> ConnectionState {
        ConnectionState::new()
    }

    /// Create dummy standard/priority ws senders for tests that don't need to
    /// inspect forwarded responses.
    fn dummy_ws_txs() -> (mpsc::Sender<GatewayResponse>, mpsc::Sender<GatewayResponse>) {
        let (standard_tx, _standard_rx) = mpsc::channel(16);
        let (priority_tx, _priority_rx) = mpsc::channel(16);
        (standard_tx, priority_tx)
    }

    fn paired_token(state: &AppState) -> String {
        let code = state.pairing.pairing_code().expect("pairing code");
        state
            .pairing
            .try_pair(&code)
            .expect("pair should succeed")
            .expect("token expected")
    }

    async fn request_json(app: Router, request: Request<Body>) -> (StatusCode, serde_json::Value) {
        let response = app.oneshot(request).await.expect("router response");
        let status = response.status();
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("response bytes");
        let json: serde_json::Value = serde_json::from_slice(&body).expect("response json");
        (status, json)
    }

    // --- Ping/Pong ---

    #[tokio::test]
    async fn test_ping_returns_pong() {
        let state = make_state(MockRunner::ok(), true);
        let mut conn = unauthed_conn();
        let (ws_tx, ws_priority_tx) = dummy_ws_txs();
        let resp = handle_message(
            GatewayMessage::Ping,
            &state,
            &mut conn,
            &ws_tx,
            &ws_priority_tx,
        )
        .await;
        assert_eq!(resp, GatewayResponse::Pong);
    }

    #[tokio::test]
    async fn test_ping_works_without_auth() {
        let state = make_state(MockRunner::ok(), true);
        let mut conn = unauthed_conn();
        let (ws_tx, ws_priority_tx) = dummy_ws_txs();
        assert!(!conn.authenticated);
        let resp = handle_message(
            GatewayMessage::Ping,
            &state,
            &mut conn,
            &ws_tx,
            &ws_priority_tx,
        )
        .await;
        assert_eq!(resp, GatewayResponse::Pong);
    }

    #[tokio::test]
    async fn test_get_diagnostics_requires_auth() {
        let state = make_state(MockRunner::ok(), true);
        let mut conn = unauthed_conn();
        let (ws_tx, ws_priority_tx) = dummy_ws_txs();
        let resp = handle_message(
            GatewayMessage::GetDiagnostics,
            &state,
            &mut conn,
            &ws_tx,
            &ws_priority_tx,
        )
        .await;
        assert_eq!(resp, auth_required_error());
    }

    #[tokio::test]
    async fn test_get_diagnostics_success() {
        let state = make_state(MockRunner::ok(), true);
        let mut conn = authed_conn();
        let (ws_tx, ws_priority_tx) = dummy_ws_txs();
        let resp = handle_message(
            GatewayMessage::GetDiagnostics,
            &state,
            &mut conn,
            &ws_tx,
            &ws_priority_tx,
        )
        .await;
        match resp {
            GatewayResponse::Diagnostics { data } => {
                assert_eq!(data.runtime_kind, "native");
                assert_eq!(data.provider_profile, "legacy");
                assert_eq!(data.compactions_applied_total, 3);
                assert_eq!(data.protocol_version, crate::protocol::PROTOCOL_VERSION);
                assert!(data
                    .supported_client_messages
                    .contains(&"GetDiagnostics".to_string()));
                assert!(data
                    .supported_server_responses
                    .contains(&"Diagnostics".to_string()));
                assert_eq!(data.security_workspace_root, ".");
                assert_eq!(
                    data.security_forbidden_tool_paths,
                    vec![".git".to_string(), "target".to_string()]
                );
            }
            other => panic!("Expected Diagnostics response, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_get_gateway_health_works_without_auth() {
        let state = make_state(MockRunner::ok(), true);
        let mut conn = unauthed_conn();
        let (ws_tx, ws_priority_tx) = dummy_ws_txs();
        let resp = handle_message(
            GatewayMessage::GetGatewayHealth,
            &state,
            &mut conn,
            &ws_tx,
            &ws_priority_tx,
        )
        .await;

        match resp {
            GatewayResponse::GatewayHealth { data } => {
                assert_eq!(data.protocol_version, crate::protocol::PROTOCOL_VERSION);
                assert!(data.require_pairing);
                assert_eq!(data.active_sessions, 0);
                assert_eq!(data.active_ws_bindings, 0);
                assert_eq!(data.active_priority_bindings, 0);
                assert_eq!(data.active_channel_jobs, 0);
            }
            other => panic!("Expected GatewayHealth response, got {:?}", other),
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn test_concurrent_create_session_burst_updates_health_and_ownership() {
        let state = make_state(MockRunner::ok(), false);
        // RC minimum verified burst capacity for concurrent CreateSession calls.
        let task_count = 128usize;
        let mut handles = Vec::with_capacity(task_count);

        for idx in 0..task_count {
            let state = state.clone();
            handles.push(tokio::spawn(async move {
                let mut conn = ConnectionState {
                    authenticated: true,
                    token: Some(format!("tok-{}", idx)),
                    principal_id: Some(format!("principal-{}", idx % 4)),
                    owned_sessions: Vec::new(),
                };
                let (ws_tx, ws_priority_tx) = dummy_ws_txs();
                handle_message(
                    GatewayMessage::CreateSession { config: None },
                    &state,
                    &mut conn,
                    &ws_tx,
                    &ws_priority_tx,
                )
                .await
            }));
        }

        let mut session_ids = std::collections::HashSet::new();
        for handle in handles {
            let response = handle.await.expect("join create-session task");
            match response {
                GatewayResponse::SessionCreated { session_id } => {
                    assert!(
                        session_ids.insert(session_id),
                        "duplicate session_id emitted during concurrent burst"
                    );
                }
                other => panic!("Expected SessionCreated, got {:?}", other),
            }
        }
        assert_eq!(session_ids.len(), task_count);
        assert_eq!(state.session_owners.read().await.len(), task_count);

        let mut conn = unauthed_conn();
        let (ws_tx, ws_priority_tx) = dummy_ws_txs();
        let health = handle_message(
            GatewayMessage::GetGatewayHealth,
            &state,
            &mut conn,
            &ws_tx,
            &ws_priority_tx,
        )
        .await;
        match health {
            GatewayResponse::GatewayHealth { data } => {
                assert_eq!(data.active_sessions, task_count);
                assert_eq!(data.active_ws_bindings, task_count);
                assert_eq!(data.active_priority_bindings, task_count);
            }
            other => panic!("Expected GatewayHealth response, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_priority_probe_requires_auth() {
        let state = make_state(MockRunner::ok(), true);
        let mut conn = unauthed_conn();
        let (ws_tx, ws_priority_tx) = dummy_ws_txs();
        let resp = handle_message(
            GatewayMessage::PriorityProbe,
            &state,
            &mut conn,
            &ws_tx,
            &ws_priority_tx,
        )
        .await;
        assert_eq!(resp, auth_required_error());
    }

    #[tokio::test]
    async fn test_priority_probe_enqueues_priority_and_standard_messages() {
        let state = make_state(MockRunner::ok(), false);
        let mut conn = authed_conn();
        conn.owned_sessions.push("sess-priority".to_string());
        let (ws_tx, mut ws_rx) = mpsc::channel(16);
        let (ws_priority_tx, mut ws_priority_rx) = mpsc::channel(16);

        let resp = handle_message(
            GatewayMessage::PriorityProbe,
            &state,
            &mut conn,
            &ws_tx,
            &ws_priority_tx,
        )
        .await;
        match resp {
            GatewayResponse::PriorityProbeAccepted {
                queued_standard,
                queued_priority,
            } => {
                assert!(queued_standard);
                assert!(queued_priority);
            }
            other => panic!("Expected PriorityProbeAccepted, got {:?}", other),
        }

        let priority = ws_priority_rx.recv().await.expect("priority notice");
        match priority {
            GatewayResponse::PriorityNotice { data } => {
                assert_eq!(data.code, "priority_probe");
                assert_eq!(data.session_id.as_deref(), Some("sess-priority"));
            }
            other => panic!("Expected PriorityNotice, got {:?}", other),
        }

        let standard = ws_rx.recv().await.expect("standard response");
        match standard {
            GatewayResponse::TranscriptUpdate {
                session_id, text, ..
            } => {
                assert_eq!(session_id, "sess-priority");
                assert_eq!(text, "priority_probe_standard");
            }
            other => panic!("Expected TranscriptUpdate, got {:?}", other),
        }
    }

    // --- Auth required for session commands ---

    #[tokio::test]
    async fn test_create_session_requires_auth() {
        let state = make_state(MockRunner::ok(), true);
        let mut conn = unauthed_conn();
        let (ws_tx, ws_priority_tx) = dummy_ws_txs();
        let resp = handle_message(
            GatewayMessage::CreateSession { config: None },
            &state,
            &mut conn,
            &ws_tx,
            &ws_priority_tx,
        )
        .await;
        match resp {
            GatewayResponse::Error { code, .. } => assert_eq!(code, "auth_required"),
            other => panic!("Expected auth_required error, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_terminate_session_requires_auth() {
        let state = make_state(MockRunner::ok(), true);
        let mut conn = unauthed_conn();
        let (ws_tx, ws_priority_tx) = dummy_ws_txs();
        let resp = handle_message(
            GatewayMessage::TerminateSession {
                session_id: "s1".into(),
            },
            &state,
            &mut conn,
            &ws_tx,
            &ws_priority_tx,
        )
        .await;
        match resp {
            GatewayResponse::Error { code, .. } => assert_eq!(code, "auth_required"),
            other => panic!("Expected auth_required error, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_session_audio_requires_auth() {
        let state = make_state(MockRunner::ok(), true);
        let mut conn = unauthed_conn();
        let (ws_tx, ws_priority_tx) = dummy_ws_txs();
        let resp = handle_message(
            GatewayMessage::SessionAudio {
                session_id: "s1".into(),
                audio: "AAAA".into(),
            },
            &state,
            &mut conn,
            &ws_tx,
            &ws_priority_tx,
        )
        .await;
        match resp {
            GatewayResponse::Error { code, .. } => assert_eq!(code, "auth_required"),
            other => panic!("Expected auth_required error, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_session_audio_commit_requires_auth() {
        let state = make_state(MockRunner::ok(), true);
        let mut conn = unauthed_conn();
        let (ws_tx, ws_priority_tx) = dummy_ws_txs();
        let resp = handle_message(
            GatewayMessage::SessionAudioCommit {
                session_id: "s1".into(),
            },
            &state,
            &mut conn,
            &ws_tx,
            &ws_priority_tx,
        )
        .await;
        match resp {
            GatewayResponse::Error { code, .. } => assert_eq!(code, "auth_required"),
            other => panic!("Expected auth_required error, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_session_response_create_requires_auth() {
        let state = make_state(MockRunner::ok(), true);
        let mut conn = unauthed_conn();
        let (ws_tx, ws_priority_tx) = dummy_ws_txs();
        let resp = handle_message(
            GatewayMessage::SessionResponseCreate {
                session_id: "s1".into(),
            },
            &state,
            &mut conn,
            &ws_tx,
            &ws_priority_tx,
        )
        .await;
        match resp {
            GatewayResponse::Error { code, .. } => assert_eq!(code, "auth_required"),
            other => panic!("Expected auth_required error, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_session_response_interrupt_requires_auth() {
        let state = make_state(MockRunner::ok(), true);
        let mut conn = unauthed_conn();
        let (ws_tx, ws_priority_tx) = dummy_ws_txs();
        let resp = handle_message(
            GatewayMessage::SessionResponseInterrupt {
                session_id: "s1".into(),
            },
            &state,
            &mut conn,
            &ws_tx,
            &ws_priority_tx,
        )
        .await;
        match resp {
            GatewayResponse::Error { code, .. } => assert_eq!(code, "auth_required"),
            other => panic!("Expected auth_required error, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_session_prompt_requires_auth() {
        let state = make_state(MockRunner::ok(), true);
        let mut conn = unauthed_conn();
        let (ws_tx, ws_priority_tx) = dummy_ws_txs();
        let resp = handle_message(
            GatewayMessage::SessionPrompt {
                session_id: "s1".into(),
                prompt: "Use add_numbers for 2 and 3".into(),
                create_response: Some(true),
            },
            &state,
            &mut conn,
            &ws_tx,
            &ws_priority_tx,
        )
        .await;
        match resp {
            GatewayResponse::Error { code, .. } => assert_eq!(code, "auth_required"),
            other => panic!("Expected auth_required error, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_session_tool_call_requires_auth() {
        let state = make_state(MockRunner::ok(), true);
        let mut conn = unauthed_conn();
        let (ws_tx, ws_priority_tx) = dummy_ws_txs();
        let resp = handle_message(
            GatewayMessage::SessionToolCall {
                session_id: "s1".into(),
                tool_name: "echo_text".into(),
                arguments: serde_json::json!({ "text": "hello" }),
            },
            &state,
            &mut conn,
            &ws_tx,
            &ws_priority_tx,
        )
        .await;
        match resp {
            GatewayResponse::Error { code, .. } => assert_eq!(code, "auth_required"),
            other => panic!("Expected auth_required error, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_channel_inbound_requires_auth() {
        let state = make_state(MockRunner::ok(), true);
        let mut conn = unauthed_conn();
        let (ws_tx, ws_priority_tx) = dummy_ws_txs();
        let resp = handle_message(
            GatewayMessage::ChannelInbound {
                channel: "telegram".into(),
                account_id: "acct-1".into(),
                external_user_id: "user-1".into(),
                text: "hello".into(),
                create_response: Some(true),
            },
            &state,
            &mut conn,
            &ws_tx,
            &ws_priority_tx,
        )
        .await;
        match resp {
            GatewayResponse::Error { code, .. } => assert_eq!(code, "auth_required"),
            other => panic!("Expected auth_required error, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_get_channel_outbound_requires_auth() {
        let state = make_state(MockRunner::ok(), true);
        let mut conn = unauthed_conn();
        let (ws_tx, ws_priority_tx) = dummy_ws_txs();
        let resp = handle_message(
            GatewayMessage::GetChannelOutbound {
                channel: "webhook".into(),
                account_id: "acct-1".into(),
                external_user_id: "user-1".into(),
                max_items: Some(10),
            },
            &state,
            &mut conn,
            &ws_tx,
            &ws_priority_tx,
        )
        .await;
        match resp {
            GatewayResponse::Error { code, .. } => assert_eq!(code, "auth_required"),
            other => panic!("Expected auth_required error, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_create_channel_job_requires_auth() {
        let state = make_state(MockRunner::ok(), true);
        let mut conn = unauthed_conn();
        let (ws_tx, ws_priority_tx) = dummy_ws_txs();
        let resp = handle_message(
            GatewayMessage::CreateChannelJob {
                channel: "webhook".into(),
                account_id: "acct-1".into(),
                external_user_id: "user-1".into(),
                text: "ping".into(),
                interval_seconds: 30,
                create_response: Some(false),
            },
            &state,
            &mut conn,
            &ws_tx,
            &ws_priority_tx,
        )
        .await;
        match resp {
            GatewayResponse::Error { code, .. } => assert_eq!(code, "auth_required"),
            other => panic!("Expected auth_required error, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_cancel_channel_job_requires_auth() {
        let state = make_state(MockRunner::ok(), true);
        let mut conn = unauthed_conn();
        let (ws_tx, ws_priority_tx) = dummy_ws_txs();
        let resp = handle_message(
            GatewayMessage::CancelChannelJob {
                job_id: "job-1".into(),
            },
            &state,
            &mut conn,
            &ws_tx,
            &ws_priority_tx,
        )
        .await;
        match resp {
            GatewayResponse::Error { code, .. } => assert_eq!(code, "auth_required"),
            other => panic!("Expected auth_required error, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_list_channel_jobs_requires_auth() {
        let state = make_state(MockRunner::ok(), true);
        let mut conn = unauthed_conn();
        let (ws_tx, ws_priority_tx) = dummy_ws_txs();
        let resp = handle_message(
            GatewayMessage::ListChannelJobs,
            &state,
            &mut conn,
            &ws_tx,
            &ws_priority_tx,
        )
        .await;
        match resp {
            GatewayResponse::Error { code, .. } => assert_eq!(code, "auth_required"),
            other => panic!("Expected auth_required error, got {:?}", other),
        }
    }

    // --- Pairing ---

    #[tokio::test]
    async fn test_pair_success_authenticates_connection() {
        let state = make_state(MockRunner::ok(), true);
        let mut conn = unauthed_conn();
        let (ws_tx, ws_priority_tx) = dummy_ws_txs();
        let code = state.pairing.pairing_code().unwrap();

        let resp = handle_message(
            GatewayMessage::Pair { code },
            &state,
            &mut conn,
            &ws_tx,
            &ws_priority_tx,
        )
        .await;
        match resp {
            GatewayResponse::PairSuccess { token } => {
                assert!(!token.is_empty());
                assert!(conn.authenticated);
            }
            other => panic!("Expected PairSuccess, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_pair_wrong_code_fails() {
        let state = make_state(MockRunner::ok(), true);
        let mut conn = unauthed_conn();
        let (ws_tx, ws_priority_tx) = dummy_ws_txs();
        let code = state.pairing.pairing_code().unwrap();
        let wrong = if code == "999999" { "000000" } else { "999999" };

        let resp = handle_message(
            GatewayMessage::Pair {
                code: wrong.to_string(),
            },
            &state,
            &mut conn,
            &ws_tx,
            &ws_priority_tx,
        )
        .await;
        match resp {
            GatewayResponse::PairFailure { .. } => {
                assert!(!conn.authenticated);
            }
            other => panic!("Expected PairFailure, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_pair_disabled_auto_authenticates() {
        let state = make_state(MockRunner::ok(), false);
        let mut conn = unauthed_conn();
        let (ws_tx, ws_priority_tx) = dummy_ws_txs();

        let resp = handle_message(
            GatewayMessage::Pair {
                code: "anything".into(),
            },
            &state,
            &mut conn,
            &ws_tx,
            &ws_priority_tx,
        )
        .await;
        match resp {
            GatewayResponse::PairSuccess { token } => {
                assert_eq!(token, "no-auth-required");
                assert!(conn.authenticated);
            }
            other => panic!("Expected PairSuccess, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_authenticate_with_valid_token() {
        let state = make_state(MockRunner::ok(), true);
        let mut conn_a = unauthed_conn();
        let (ws_tx_a, ws_priority_tx_a) = dummy_ws_txs();

        let code = state.pairing.pairing_code().unwrap();
        let token = match handle_message(
            GatewayMessage::Pair { code },
            &state,
            &mut conn_a,
            &ws_tx_a,
            &ws_priority_tx_a,
        )
        .await
        {
            GatewayResponse::PairSuccess { token } => token,
            other => panic!("Expected PairSuccess, got {:?}", other),
        };

        let mut conn_b = unauthed_conn();
        let (ws_tx_b, ws_priority_tx_b) = dummy_ws_txs();
        let resp = handle_message(
            GatewayMessage::Authenticate {
                token: token.clone(),
            },
            &state,
            &mut conn_b,
            &ws_tx_b,
            &ws_priority_tx_b,
        )
        .await;

        match resp {
            GatewayResponse::Authenticated { principal_id } => {
                assert_eq!(principal_id, principal_from_token(&token));
                assert!(conn_b.authenticated);
            }
            other => panic!("Expected Authenticated, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_authenticate_rejects_invalid_token() {
        let state = make_state(MockRunner::ok(), true);
        let mut conn = unauthed_conn();
        let (ws_tx, ws_priority_tx) = dummy_ws_txs();

        let resp = handle_message(
            GatewayMessage::Authenticate {
                token: "invalid-token".to_string(),
            },
            &state,
            &mut conn,
            &ws_tx,
            &ws_priority_tx,
        )
        .await;

        match resp {
            GatewayResponse::Error { code, .. } => {
                assert_eq!(code, "invalid_token");
                assert!(!conn.authenticated);
            }
            other => panic!("Expected invalid_token error, got {:?}", other),
        }
    }

    // --- Session creation ---

    #[tokio::test]
    async fn test_create_session_success() {
        let state = make_state(MockRunner::ok(), false);
        let mut conn = authed_conn();
        let (ws_tx, ws_priority_tx) = dummy_ws_txs();

        let resp = handle_message(
            GatewayMessage::CreateSession { config: None },
            &state,
            &mut conn,
            &ws_tx,
            &ws_priority_tx,
        )
        .await;
        match resp {
            GatewayResponse::SessionCreated { session_id } => {
                assert!(!session_id.is_empty());
                // Verify ws_response_sender was registered
                assert!(state
                    .ws_response_senders
                    .read()
                    .await
                    .contains_key(&session_id));
                // Verify session is tracked in owned_sessions
                assert!(conn.owned_sessions.contains(&session_id));
            }
            other => panic!("Expected SessionCreated, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_create_session_with_config() {
        let state = make_state(MockRunner::ok(), false);
        let mut conn = authed_conn();
        let (ws_tx, ws_priority_tx) = dummy_ws_txs();
        let config = SessionConfig {
            model: Some("gpt-4o".into()),
            voice: Some("alloy".into()),
            instructions: None,
            role: Some("supervised".into()),
            enable_graph: Some(true),
        };

        let resp = handle_message(
            GatewayMessage::CreateSession {
                config: Some(config),
            },
            &state,
            &mut conn,
            &ws_tx,
            &ws_priority_tx,
        )
        .await;
        match resp {
            GatewayResponse::SessionCreated { session_id } => {
                assert!(!session_id.is_empty());
            }
            other => panic!("Expected SessionCreated, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_create_session_runner_error() {
        let state = make_state(MockRunner::with_create_err("provider unavailable"), false);
        let mut conn = authed_conn();
        let (ws_tx, ws_priority_tx) = dummy_ws_txs();

        let resp = handle_message(
            GatewayMessage::CreateSession { config: None },
            &state,
            &mut conn,
            &ws_tx,
            &ws_priority_tx,
        )
        .await;
        match resp {
            GatewayResponse::Error { code, message } => {
                assert_eq!(code, "session_create_failed");
                assert!(message.contains("provider unavailable"));
            }
            other => panic!("Expected Error, got {:?}", other),
        }
    }

    // --- Session termination ---

    #[tokio::test]
    async fn test_terminate_session_success() {
        let state = make_state(MockRunner::ok(), false);
        let mut conn = authed_conn();
        let (ws_tx, ws_priority_tx) = dummy_ws_txs();
        conn.owned_sessions.push("sess-1".to_string());

        let resp = handle_message(
            GatewayMessage::TerminateSession {
                session_id: "sess-1".into(),
            },
            &state,
            &mut conn,
            &ws_tx,
            &ws_priority_tx,
        )
        .await;
        match resp {
            GatewayResponse::SessionTerminated { session_id } => {
                assert_eq!(session_id, "sess-1");
            }
            other => panic!("Expected SessionTerminated, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_terminate_session_cleans_up_audio_sender() {
        let state = make_state(MockRunner::ok(), false);
        let mut conn = authed_conn();
        let (ws_tx, ws_priority_tx) = dummy_ws_txs();

        // Insert a dummy audio sender
        let (tx, _rx) = mpsc::channel(1);
        state
            .audio_senders
            .write()
            .await
            .insert("sess-1".to_string(), tx);

        // Also register a ws_response_sender
        let (ws_resp_tx, _ws_resp_rx) = mpsc::channel(1);
        state
            .ws_response_senders
            .write()
            .await
            .insert("sess-1".to_string(), ws_resp_tx);
        state
            .session_owners
            .write()
            .await
            .insert("sess-1".to_string(), "user-1".to_string());
        conn.owned_sessions.push("sess-1".to_string());

        let resp = handle_message(
            GatewayMessage::TerminateSession {
                session_id: "sess-1".into(),
            },
            &state,
            &mut conn,
            &ws_tx,
            &ws_priority_tx,
        )
        .await;
        assert!(matches!(resp, GatewayResponse::SessionTerminated { .. }));
        assert!(!state.audio_senders.read().await.contains_key("sess-1"));
        assert!(!state
            .ws_response_senders
            .read()
            .await
            .contains_key("sess-1"));
        assert!(!state.session_owners.read().await.contains_key("sess-1"));
        assert!(!conn.owned_sessions.contains(&"sess-1".to_string()));
    }

    // --- Session audio ---

    #[tokio::test]
    async fn test_session_audio_forwards_to_runner() {
        let state = make_state(MockRunner::ok(), false);
        let mut conn = authed_conn();
        let (ws_tx, ws_priority_tx) = dummy_ws_txs();
        conn.owned_sessions.push("sess-1".to_string());

        let resp = handle_message(
            GatewayMessage::SessionAudio {
                session_id: "sess-1".into(),
                audio: "AQID".into(), // base64 for [1, 2, 3]
            },
            &state,
            &mut conn,
            &ws_tx,
            &ws_priority_tx,
        )
        .await;
        match resp {
            GatewayResponse::AudioAccepted { session_id } => {
                assert_eq!(session_id, "sess-1");
            }
            other => panic!("Expected AudioAccepted, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_session_audio_commit_forwards_to_runner() {
        let state = make_state(MockRunner::ok(), false);
        let mut conn = authed_conn();
        let (ws_tx, ws_priority_tx) = dummy_ws_txs();
        conn.owned_sessions.push("sess-1".to_string());

        let resp = handle_message(
            GatewayMessage::SessionAudioCommit {
                session_id: "sess-1".into(),
            },
            &state,
            &mut conn,
            &ws_tx,
            &ws_priority_tx,
        )
        .await;
        match resp {
            GatewayResponse::AudioCommitted { session_id } => {
                assert_eq!(session_id, "sess-1");
            }
            other => panic!("Expected AudioCommitted, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_session_response_create_forwards_to_runner() {
        let state = make_state(MockRunner::ok(), false);
        let mut conn = authed_conn();
        let (ws_tx, ws_priority_tx) = dummy_ws_txs();
        conn.owned_sessions.push("sess-1".to_string());

        let resp = handle_message(
            GatewayMessage::SessionResponseCreate {
                session_id: "sess-1".into(),
            },
            &state,
            &mut conn,
            &ws_tx,
            &ws_priority_tx,
        )
        .await;
        match resp {
            GatewayResponse::ResponseCreateAccepted { session_id } => {
                assert_eq!(session_id, "sess-1");
            }
            other => panic!("Expected ResponseCreateAccepted, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_session_response_interrupt_forwards_to_runner() {
        let state = make_state(MockRunner::ok(), false);
        let mut conn = authed_conn();
        let (ws_tx, ws_priority_tx) = dummy_ws_txs();
        conn.owned_sessions.push("sess-1".to_string());

        let resp = handle_message(
            GatewayMessage::SessionResponseInterrupt {
                session_id: "sess-1".into(),
            },
            &state,
            &mut conn,
            &ws_tx,
            &ws_priority_tx,
        )
        .await;
        match resp {
            GatewayResponse::ResponseInterruptAccepted { session_id } => {
                assert_eq!(session_id, "sess-1");
            }
            other => panic!("Expected ResponseInterruptAccepted, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_session_prompt_forwards_to_runner() {
        let state = make_state(MockRunner::ok(), false);
        let mut conn = authed_conn();
        let (ws_tx, ws_priority_tx) = dummy_ws_txs();
        conn.owned_sessions.push("sess-1".to_string());

        let resp = handle_message(
            GatewayMessage::SessionPrompt {
                session_id: "sess-1".into(),
                prompt: "Use utc_time and return UTC".into(),
                create_response: Some(true),
            },
            &state,
            &mut conn,
            &ws_tx,
            &ws_priority_tx,
        )
        .await;
        match resp {
            GatewayResponse::PromptAccepted { session_id } => {
                assert_eq!(session_id, "sess-1");
            }
            other => panic!("Expected PromptAccepted, got {:?}", other),
        }
    }

    #[test]
    fn test_maybe_extract_workspace_read_path_defaults_to_readme() {
        let path = maybe_extract_workspace_read_path("read the readme file and summarize");
        assert_eq!(path.as_deref(), Some("README.md"));
    }

    #[tokio::test]
    async fn test_session_prompt_readme_bridges_tool_call() {
        let runner = MockRunner::ok();
        let tool_calls = runner.tool_calls.clone();
        let last_prompt = runner.last_prompt.clone();
        let state = make_state(runner, false);

        let mut conn = authed_conn();
        let (ws_tx, ws_priority_tx) = dummy_ws_txs();
        conn.owned_sessions.push("sess-1".to_string());

        let resp = handle_message(
            GatewayMessage::SessionPrompt {
                session_id: "sess-1".into(),
                prompt: "read the readme file and give me a summary".into(),
                create_response: Some(true),
            },
            &state,
            &mut conn,
            &ws_tx,
            &ws_priority_tx,
        )
        .await;

        match resp {
            GatewayResponse::PromptAccepted { session_id } => {
                assert_eq!(session_id, "sess-1");
            }
            other => panic!("Expected PromptAccepted, got {:?}", other),
        }

        let calls = tool_calls.lock().expect("tool_calls mutex poisoned");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "read_workspace_file");
        assert_eq!(calls[0].1["path"], serde_json::json!("README.md"));

        let prompt = last_prompt
            .lock()
            .expect("last_prompt mutex poisoned")
            .clone()
            .expect("expected send_text prompt");
        assert!(prompt.contains("Context from read_workspace_file"));
        assert!(prompt.contains("README.md"));
    }

    #[tokio::test]
    async fn test_session_tool_call_forwards_to_runner() {
        let state = make_state(MockRunner::ok(), false);
        let mut conn = authed_conn();
        let (ws_tx, ws_priority_tx) = dummy_ws_txs();
        conn.owned_sessions.push("sess-1".to_string());

        let resp = handle_message(
            GatewayMessage::SessionToolCall {
                session_id: "sess-1".into(),
                tool_name: "echo_text".into(),
                arguments: serde_json::json!({ "text": "hello" }),
            },
            &state,
            &mut conn,
            &ws_tx,
            &ws_priority_tx,
        )
        .await;
        match resp {
            GatewayResponse::SessionToolResult {
                session_id,
                tool_name,
                result,
                graph,
            } => {
                assert_eq!(session_id, "sess-1");
                assert_eq!(tool_name, "echo_text");
                assert_eq!(result["status"], serde_json::json!("ok"));
                assert!(graph.completed);
                assert!(!graph.interrupted);
            }
            other => panic!("Expected SessionToolResult, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_session_audio_invalid_base64() {
        let state = make_state(MockRunner::ok(), false);
        let mut conn = authed_conn();
        let (ws_tx, ws_priority_tx) = dummy_ws_txs();
        conn.owned_sessions.push("sess-1".to_string());

        let resp = handle_message(
            GatewayMessage::SessionAudio {
                session_id: "sess-1".into(),
                audio: "!!!invalid!!!".into(),
            },
            &state,
            &mut conn,
            &ws_tx,
            &ws_priority_tx,
        )
        .await;
        match resp {
            GatewayResponse::Error { code, .. } => assert_eq!(code, "invalid_audio"),
            other => panic!("Expected invalid_audio error, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_session_audio_rejects_unowned_session() {
        let state = make_state(MockRunner::ok(), false);
        let mut owner = authed_conn();
        let mut intruder = ConnectionState {
            authenticated: true,
            token: Some("tok-2".to_string()),
            principal_id: Some("user-2".to_string()),
            owned_sessions: Vec::new(),
        };
        let (ws_tx, ws_priority_tx) = dummy_ws_txs();

        // Owner creates a session and gets ownership.
        let sid = match handle_message(
            GatewayMessage::CreateSession { config: None },
            &state,
            &mut owner,
            &ws_tx,
            &ws_priority_tx,
        )
        .await
        {
            GatewayResponse::SessionCreated { session_id } => session_id,
            other => panic!("Expected SessionCreated, got {:?}", other),
        };

        let resp = handle_message(
            GatewayMessage::SessionAudio {
                session_id: sid,
                audio: "AQID".to_string(),
            },
            &state,
            &mut intruder,
            &ws_tx,
            &ws_priority_tx,
        )
        .await;

        match resp {
            GatewayResponse::Error { code, .. } => assert_eq!(code, "forbidden_session"),
            other => panic!("Expected forbidden_session, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_terminate_rejects_unowned_session() {
        let state = make_state(MockRunner::ok(), false);
        let mut owner = authed_conn();
        let mut intruder = ConnectionState {
            authenticated: true,
            token: Some("tok-2".to_string()),
            principal_id: Some("user-2".to_string()),
            owned_sessions: Vec::new(),
        };
        let (ws_tx, ws_priority_tx) = dummy_ws_txs();

        let sid = match handle_message(
            GatewayMessage::CreateSession { config: None },
            &state,
            &mut owner,
            &ws_tx,
            &ws_priority_tx,
        )
        .await
        {
            GatewayResponse::SessionCreated { session_id } => session_id,
            other => panic!("Expected SessionCreated, got {:?}", other),
        };

        let resp = handle_message(
            GatewayMessage::TerminateSession { session_id: sid },
            &state,
            &mut intruder,
            &ws_tx,
            &ws_priority_tx,
        )
        .await;

        match resp {
            GatewayResponse::Error { code, .. } => assert_eq!(code, "forbidden_session"),
            other => panic!("Expected forbidden_session, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_token_auth_can_resume_session_control() {
        let state = make_state(MockRunner::ok(), true);
        let (ws_tx, ws_priority_tx) = dummy_ws_txs();

        let mut conn_a = unauthed_conn();
        let code = state.pairing.pairing_code().unwrap();
        let token = match handle_message(
            GatewayMessage::Pair { code },
            &state,
            &mut conn_a,
            &ws_tx,
            &ws_priority_tx,
        )
        .await
        {
            GatewayResponse::PairSuccess { token } => token,
            other => panic!("Expected PairSuccess, got {:?}", other),
        };

        let sid = match handle_message(
            GatewayMessage::CreateSession { config: None },
            &state,
            &mut conn_a,
            &ws_tx,
            &ws_priority_tx,
        )
        .await
        {
            GatewayResponse::SessionCreated { session_id } => session_id,
            other => panic!("Expected SessionCreated, got {:?}", other),
        };

        // New connection authenticates with same token and can control the session.
        let mut conn_b = unauthed_conn();
        let auth_resp = handle_message(
            GatewayMessage::Authenticate {
                token: token.clone(),
            },
            &state,
            &mut conn_b,
            &ws_tx,
            &ws_priority_tx,
        )
        .await;
        assert!(matches!(auth_resp, GatewayResponse::Authenticated { .. }));

        let audio_resp = handle_message(
            GatewayMessage::SessionAudio {
                session_id: sid.clone(),
                audio: "AQID".to_string(),
            },
            &state,
            &mut conn_b,
            &ws_tx,
            &ws_priority_tx,
        )
        .await;
        assert!(matches!(audio_resp, GatewayResponse::AudioAccepted { .. }));

        let terminate_resp = handle_message(
            GatewayMessage::TerminateSession { session_id: sid },
            &state,
            &mut conn_b,
            &ws_tx,
            &ws_priority_tx,
        )
        .await;
        assert!(matches!(
            terminate_resp,
            GatewayResponse::SessionTerminated { .. }
        ));
    }

    #[tokio::test]
    async fn test_channel_inbound_reuses_session_for_same_route_key() {
        let state = make_state(MockRunner::ok(), false);
        let mut conn = authed_conn();
        let (ws_tx, ws_priority_tx) = dummy_ws_txs();

        let first = handle_message(
            GatewayMessage::ChannelInbound {
                channel: "telegram".into(),
                account_id: "acct-1".into(),
                external_user_id: "user-77".into(),
                text: "first".into(),
                create_response: Some(false),
            },
            &state,
            &mut conn,
            &ws_tx,
            &ws_priority_tx,
        )
        .await;
        let first_session_id = match first {
            GatewayResponse::ChannelRouted { session_id, .. } => session_id,
            other => panic!("Expected ChannelRouted, got {:?}", other),
        };

        let second = handle_message(
            GatewayMessage::ChannelInbound {
                channel: "TeLeGrAm".into(),
                account_id: "acct-1".into(),
                external_user_id: "user-77".into(),
                text: "second".into(),
                create_response: Some(false),
            },
            &state,
            &mut conn,
            &ws_tx,
            &ws_priority_tx,
        )
        .await;
        let second_session_id = match second {
            GatewayResponse::ChannelRouted { session_id, .. } => session_id,
            other => panic!("Expected ChannelRouted, got {:?}", other),
        };

        assert_eq!(first_session_id, second_session_id);
        assert_eq!(state.channel_routes.read().await.len(), 1);
        assert!(conn.owned_sessions.contains(&first_session_id));
    }

    #[tokio::test]
    async fn test_channel_inbound_isolates_channel_and_account_routes() {
        let state = make_state(MockRunner::ok(), false);
        let mut conn = authed_conn();
        let (ws_tx, ws_priority_tx) = dummy_ws_txs();

        let route = |channel: &str, account_id: &str, external_user_id: &str, text: &str| {
            GatewayMessage::ChannelInbound {
                channel: channel.to_string(),
                account_id: account_id.to_string(),
                external_user_id: external_user_id.to_string(),
                text: text.to_string(),
                create_response: Some(false),
            }
        };

        let sid_telegram_a = match handle_message(
            route("telegram", "acct-a", "user-1", "hello a1"),
            &state,
            &mut conn,
            &ws_tx,
            &ws_priority_tx,
        )
        .await
        {
            GatewayResponse::ChannelRouted { session_id, .. } => session_id,
            other => panic!("Expected ChannelRouted, got {:?}", other),
        };
        let sid_slack_a = match handle_message(
            route("slack", "acct-a", "user-1", "hello a1 slack"),
            &state,
            &mut conn,
            &ws_tx,
            &ws_priority_tx,
        )
        .await
        {
            GatewayResponse::ChannelRouted { session_id, .. } => session_id,
            other => panic!("Expected ChannelRouted, got {:?}", other),
        };
        let sid_telegram_b = match handle_message(
            route("telegram", "acct-b", "user-1", "hello b1"),
            &state,
            &mut conn,
            &ws_tx,
            &ws_priority_tx,
        )
        .await
        {
            GatewayResponse::ChannelRouted { session_id, .. } => session_id,
            other => panic!("Expected ChannelRouted, got {:?}", other),
        };
        let sid_telegram_a_user2 = match handle_message(
            route("telegram", "acct-a", "user-2", "hello a2"),
            &state,
            &mut conn,
            &ws_tx,
            &ws_priority_tx,
        )
        .await
        {
            GatewayResponse::ChannelRouted { session_id, .. } => session_id,
            other => panic!("Expected ChannelRouted, got {:?}", other),
        };

        assert_ne!(sid_telegram_a, sid_slack_a);
        assert_ne!(sid_telegram_a, sid_telegram_b);
        assert_ne!(sid_telegram_a, sid_telegram_a_user2);
        assert_eq!(state.channel_routes.read().await.len(), 4);
    }

    #[tokio::test]
    async fn test_get_channel_outbound_returns_queued_final_transcript_items() {
        let state = make_state(MockRunner::ok(), false);
        let mut conn = authed_conn();
        let (ws_tx, ws_priority_tx) = dummy_ws_txs();

        let session_id = match handle_message(
            GatewayMessage::ChannelInbound {
                channel: "webhook".into(),
                account_id: "acct-1".into(),
                external_user_id: "user-1".into(),
                text: "hello inbound".into(),
                create_response: Some(false),
            },
            &state,
            &mut conn,
            &ws_tx,
            &ws_priority_tx,
        )
        .await
        {
            GatewayResponse::ChannelRouted { session_id, .. } => session_id,
            other => panic!("Expected ChannelRouted, got {:?}", other),
        };

        queue_channel_outbound_transcript(
            &state,
            &SessionTranscriptOutput {
                session_id: session_id.clone(),
                text: "draft".to_string(),
                is_final: false,
            },
        )
        .await;
        queue_channel_outbound_transcript(
            &state,
            &SessionTranscriptOutput {
                session_id: session_id.clone(),
                text: "final answer".to_string(),
                is_final: true,
            },
        )
        .await;

        let first_poll = handle_message(
            GatewayMessage::GetChannelOutbound {
                channel: "webhook".into(),
                account_id: "acct-1".into(),
                external_user_id: "user-1".into(),
                max_items: Some(10),
            },
            &state,
            &mut conn,
            &ws_tx,
            &ws_priority_tx,
        )
        .await;
        match first_poll {
            GatewayResponse::ChannelOutboundBatch { items, .. } => {
                assert_eq!(items.len(), 1);
                assert_eq!(items[0].session_id, session_id);
                assert_eq!(items[0].text, "final answer");
            }
            other => panic!("Expected ChannelOutboundBatch, got {:?}", other),
        }

        let second_poll = handle_message(
            GatewayMessage::GetChannelOutbound {
                channel: "webhook".into(),
                account_id: "acct-1".into(),
                external_user_id: "user-1".into(),
                max_items: Some(10),
            },
            &state,
            &mut conn,
            &ws_tx,
            &ws_priority_tx,
        )
        .await;
        match second_poll {
            GatewayResponse::ChannelOutboundBatch { items, .. } => {
                assert!(items.is_empty());
            }
            other => panic!("Expected ChannelOutboundBatch, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_terminate_session_cleans_channel_routes() {
        let state = make_state(MockRunner::ok(), false);
        let mut conn = authed_conn();
        let (ws_tx, ws_priority_tx) = dummy_ws_txs();

        let session_id = match handle_message(
            GatewayMessage::ChannelInbound {
                channel: "webhook".into(),
                account_id: "integration-1".into(),
                external_user_id: "origin-1".into(),
                text: "incoming webhook".into(),
                create_response: Some(false),
            },
            &state,
            &mut conn,
            &ws_tx,
            &ws_priority_tx,
        )
        .await
        {
            GatewayResponse::ChannelRouted { session_id, .. } => session_id,
            other => panic!("Expected ChannelRouted, got {:?}", other),
        };
        assert_eq!(state.channel_routes.read().await.len(), 1);

        let terminate_resp = handle_message(
            GatewayMessage::TerminateSession { session_id },
            &state,
            &mut conn,
            &ws_tx,
            &ws_priority_tx,
        )
        .await;
        assert!(matches!(
            terminate_resp,
            GatewayResponse::SessionTerminated { .. }
        ));
        assert!(state.channel_routes.read().await.is_empty());
    }

    #[tokio::test]
    async fn test_channel_job_lifecycle_create_list_cancel() {
        let state = make_state(MockRunner::ok(), false);
        let mut conn = authed_conn();
        let (ws_tx, ws_priority_tx) = dummy_ws_txs();

        let created = handle_message(
            GatewayMessage::CreateChannelJob {
                channel: "webhook".into(),
                account_id: "acct-ops".into(),
                external_user_id: "user-ops".into(),
                text: "scheduled ping".into(),
                interval_seconds: 60,
                create_response: Some(false),
            },
            &state,
            &mut conn,
            &ws_tx,
            &ws_priority_tx,
        )
        .await;
        let job = match created {
            GatewayResponse::ChannelJobCreated { job } => job,
            other => panic!("Expected ChannelJobCreated, got {:?}", other),
        };

        let listed = handle_message(
            GatewayMessage::ListChannelJobs,
            &state,
            &mut conn,
            &ws_tx,
            &ws_priority_tx,
        )
        .await;
        match listed {
            GatewayResponse::ChannelJobs { jobs } => {
                assert_eq!(jobs.len(), 1);
                assert_eq!(jobs[0].job_id, job.job_id);
                assert_eq!(jobs[0].channel, "webhook");
            }
            other => panic!("Expected ChannelJobs, got {:?}", other),
        }

        let health = handle_message(
            GatewayMessage::GetGatewayHealth,
            &state,
            &mut conn,
            &ws_tx,
            &ws_priority_tx,
        )
        .await;
        match health {
            GatewayResponse::GatewayHealth { data } => {
                assert_eq!(data.active_channel_jobs, 1);
            }
            other => panic!("Expected GatewayHealth, got {:?}", other),
        }

        let canceled = handle_message(
            GatewayMessage::CancelChannelJob {
                job_id: job.job_id.clone(),
            },
            &state,
            &mut conn,
            &ws_tx,
            &ws_priority_tx,
        )
        .await;
        match canceled {
            GatewayResponse::ChannelJobCanceled { job_id } => {
                assert_eq!(job_id, job.job_id);
            }
            other => panic!("Expected ChannelJobCanceled, got {:?}", other),
        }

        let listed_after = handle_message(
            GatewayMessage::ListChannelJobs,
            &state,
            &mut conn,
            &ws_tx,
            &ws_priority_tx,
        )
        .await;
        match listed_after {
            GatewayResponse::ChannelJobs { jobs } => {
                assert!(jobs.is_empty());
            }
            other => panic!("Expected ChannelJobs, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_channel_job_tick_routes_text_for_owner_principal() {
        let runner = MockRunner::ok();
        let last_prompt = runner.last_prompt.clone();
        let state = make_state(runner, false);
        let job = ChannelJob {
            job_id: "job-cron-1".to_string(),
            channel: "webhook".to_string(),
            account_id: "acct-cron".to_string(),
            external_user_id: "user-cron".to_string(),
            text: "scheduled ping".to_string(),
            interval_seconds: 60,
            create_response: false,
            created_at_unix_ms: now_unix_ms(),
        };

        run_channel_job_tick(&state, &job, "principal-cron").await;

        let prompt = last_prompt
            .lock()
            .expect("last_prompt mutex poisoned")
            .clone();
        assert_eq!(prompt.as_deref(), Some("scheduled ping"));

        let route_key = ChannelRouteKey {
            principal_id: "principal-cron".to_string(),
            channel: "webhook".to_string(),
            account_id: "acct-cron".to_string(),
            external_user_id: "user-cron".to_string(),
        };
        let session_id = state
            .channel_routes
            .read()
            .await
            .get(&route_key)
            .cloned()
            .expect("channel route should be created by tick");
        assert_eq!(
            state
                .session_owners
                .read()
                .await
                .get(&session_id)
                .map(String::as_str),
            Some("principal-cron")
        );
    }

    #[tokio::test]
    async fn test_channel_job_tick_honors_create_response_flag() {
        let runner = MockRunner::ok();
        let response_create_calls = runner.response_create_calls.clone();
        let state = make_state(runner, false);
        let no_response_job = ChannelJob {
            job_id: "job-cron-noresp".to_string(),
            channel: "webhook".to_string(),
            account_id: "acct-cron".to_string(),
            external_user_id: "user-cron".to_string(),
            text: "scheduled ping".to_string(),
            interval_seconds: 60,
            create_response: false,
            created_at_unix_ms: now_unix_ms(),
        };
        let with_response_job = ChannelJob {
            job_id: "job-cron-resp".to_string(),
            channel: "webhook".to_string(),
            account_id: "acct-cron".to_string(),
            external_user_id: "user-cron".to_string(),
            text: "scheduled ping".to_string(),
            interval_seconds: 60,
            create_response: true,
            created_at_unix_ms: now_unix_ms(),
        };

        run_channel_job_tick(&state, &no_response_job, "principal-cron").await;
        assert_eq!(
            *response_create_calls
                .lock()
                .expect("response_create_calls mutex poisoned"),
            0
        );

        run_channel_job_tick(&state, &with_response_job, "principal-cron").await;
        assert_eq!(
            *response_create_calls
                .lock()
                .expect("response_create_calls mutex poisoned"),
            1
        );
    }

    #[tokio::test]
    async fn test_create_channel_job_rejects_zero_interval() {
        let state = make_state(MockRunner::ok(), false);
        let mut conn = authed_conn();
        let (ws_tx, ws_priority_tx) = dummy_ws_txs();

        let resp = handle_message(
            GatewayMessage::CreateChannelJob {
                channel: "webhook".into(),
                account_id: "acct-1".into(),
                external_user_id: "user-1".into(),
                text: "scheduled ping".into(),
                interval_seconds: 0,
                create_response: Some(false),
            },
            &state,
            &mut conn,
            &ws_tx,
            &ws_priority_tx,
        )
        .await;
        match resp {
            GatewayResponse::Error { code, .. } => assert_eq!(code, "invalid_channel_job"),
            other => panic!("Expected invalid_channel_job error, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_cancel_channel_job_rejects_non_owner() {
        let state = make_state(MockRunner::ok(), false);
        let mut owner_conn = authed_conn();
        let mut intruder_conn = ConnectionState {
            authenticated: true,
            token: Some("tok-2".to_string()),
            principal_id: Some("user-2".to_string()),
            owned_sessions: Vec::new(),
        };
        let (ws_tx, ws_priority_tx) = dummy_ws_txs();

        let job_id = match handle_message(
            GatewayMessage::CreateChannelJob {
                channel: "webhook".into(),
                account_id: "acct-sec".into(),
                external_user_id: "user-sec".into(),
                text: "scheduled ping".into(),
                interval_seconds: 60,
                create_response: Some(false),
            },
            &state,
            &mut owner_conn,
            &ws_tx,
            &ws_priority_tx,
        )
        .await
        {
            GatewayResponse::ChannelJobCreated { job } => job.job_id,
            other => panic!("Expected ChannelJobCreated, got {:?}", other),
        };

        let forbidden = handle_message(
            GatewayMessage::CancelChannelJob {
                job_id: job_id.clone(),
            },
            &state,
            &mut intruder_conn,
            &ws_tx,
            &ws_priority_tx,
        )
        .await;
        match forbidden {
            GatewayResponse::Error { code, .. } => assert_eq!(code, "forbidden_channel_job"),
            other => panic!("Expected forbidden_channel_job, got {:?}", other),
        }

        let owner_cancel = handle_message(
            GatewayMessage::CancelChannelJob { job_id },
            &state,
            &mut owner_conn,
            &ws_tx,
            &ws_priority_tx,
        )
        .await;
        assert!(matches!(
            owner_cancel,
            GatewayResponse::ChannelJobCanceled { .. }
        ));
    }

    #[tokio::test]
    async fn test_channel_webhook_http_requires_auth_token() {
        let state = make_state(MockRunner::ok(), true);
        let app = build_router(state);

        let request = Request::builder()
            .method("POST")
            .uri("/channels/webhook")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                r#"{"channel":"webhook","account_id":"acct-1","external_user_id":"user-1","text":"hello"}"#,
            ))
            .expect("request");

        let (status, body) = request_json(app, request).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body["code"], serde_json::json!("auth_required"));
    }

    #[tokio::test]
    async fn test_channel_webhook_http_routes_with_bearer_token() {
        let state = make_state(MockRunner::ok(), true);
        let token = paired_token(&state);
        let app = build_router(state);

        let request = Request::builder()
            .method("POST")
            .uri("/channels/webhook")
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::AUTHORIZATION, format!("Bearer {}", token))
            .body(Body::from(
                r#"{"channel":"webhook","account_id":"acct-1","external_user_id":"user-1","text":"hello webhook"}"#,
            ))
            .expect("request");

        let (status, body) = request_json(app, request).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["type"], serde_json::json!("ChannelRouted"));
        assert_eq!(body["channel"], serde_json::json!("webhook"));
        assert!(body["session_id"].as_str().is_some());
    }

    #[tokio::test]
    async fn test_channel_slack_http_routes_message_event() {
        let state = make_state(MockRunner::ok(), true);
        let token = paired_token(&state);
        let app = build_router(state);

        let request = Request::builder()
            .method("POST")
            .uri("/channels/slack/events")
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::AUTHORIZATION, format!("Bearer {}", token))
            .body(Body::from(
                r#"{"team_id":"T123","event":{"type":"message","user":"U77","text":"hello from slack"}}"#,
            ))
            .expect("request");

        let (status, body) = request_json(app, request).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["type"], serde_json::json!("ChannelRouted"));
        assert_eq!(body["channel"], serde_json::json!("slack"));
        assert_eq!(body["account_id"], serde_json::json!("T123"));
        assert_eq!(body["external_user_id"], serde_json::json!("U77"));
    }

    #[tokio::test]
    async fn test_channel_slack_http_url_verification_returns_challenge() {
        let state = make_state(MockRunner::ok(), true);
        let app = build_router(state);

        let request = Request::builder()
            .method("POST")
            .uri("/channels/slack/events")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                r#"{"type":"url_verification","challenge":"abc123"}"#,
            ))
            .expect("request");

        let (status, body) = request_json(app, request).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["challenge"], serde_json::json!("abc123"));
    }

    #[tokio::test]
    async fn test_channel_telegram_http_routes_message() {
        let state = make_state(MockRunner::ok(), true);
        let token = paired_token(&state);
        let app = build_router(state);

        let request = Request::builder()
            .method("POST")
            .uri("/channels/telegram/update")
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::AUTHORIZATION, format!("Bearer {}", token))
            .body(Body::from(
                r#"{"message":{"text":"hello from telegram","chat":{"id":4455},"from":{"id":7788}}}"#,
            ))
            .expect("request");

        let (status, body) = request_json(app, request).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["type"], serde_json::json!("ChannelRouted"));
        assert_eq!(body["channel"], serde_json::json!("telegram"));
        assert_eq!(body["account_id"], serde_json::json!("4455"));
        assert_eq!(body["external_user_id"], serde_json::json!("7788"));
    }

    #[tokio::test]
    async fn test_channel_outbound_poll_http_requires_auth_token() {
        let state = make_state(MockRunner::ok(), true);
        let app = build_router(state);

        let request = Request::builder()
            .method("POST")
            .uri("/channels/outbound/poll")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                r#"{"channel":"webhook","account_id":"acct-1","external_user_id":"user-1","max_items":10}"#,
            ))
            .expect("request");

        let (status, body) = request_json(app, request).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body["code"], serde_json::json!("auth_required"));
    }

    #[tokio::test]
    async fn test_channel_outbound_poll_http_returns_and_drains_items() {
        let state = make_state(MockRunner::ok(), true);
        let token = paired_token(&state);
        let route_key = ChannelRouteKey {
            principal_id: principal_from_token(&token),
            channel: "webhook".to_string(),
            account_id: "acct-1".to_string(),
            external_user_id: "user-1".to_string(),
        };
        state.channel_outbox.write().await.insert(
            route_key,
            vec![ChannelOutboundItem {
                id: "evt-1".to_string(),
                session_id: "sess-1".to_string(),
                text: "outbound text".to_string(),
                created_at_unix_ms: 1,
            }],
        );
        let app = build_router(state);

        let request = Request::builder()
            .method("POST")
            .uri("/channels/outbound/poll")
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::AUTHORIZATION, format!("Bearer {}", token))
            .body(Body::from(
                r#"{"channel":"webhook","account_id":"acct-1","external_user_id":"user-1","max_items":10}"#,
            ))
            .expect("request");

        let (status, body) = request_json(app.clone(), request).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["type"], serde_json::json!("ChannelOutboundBatch"));
        assert_eq!(body["items"].as_array().map(|v| v.len()), Some(1));
        assert_eq!(body["items"][0]["text"], serde_json::json!("outbound text"));

        let second_request = Request::builder()
            .method("POST")
            .uri("/channels/outbound/poll")
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::AUTHORIZATION, format!("Bearer {}", token))
            .body(Body::from(
                r#"{"channel":"webhook","account_id":"acct-1","external_user_id":"user-1","max_items":10}"#,
            ))
            .expect("request");
        let (second_status, second_body) = request_json(app, second_request).await;
        assert_eq!(second_status, StatusCode::OK);
        assert_eq!(second_body["items"].as_array().map(|v| v.len()), Some(0));
    }

    #[tokio::test]
    async fn test_channel_job_create_http_requires_auth_token() {
        let state = make_state(MockRunner::ok(), true);
        let app = build_router(state);

        let request = Request::builder()
            .method("POST")
            .uri("/channels/jobs/create")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                r#"{"channel":"webhook","account_id":"acct-1","external_user_id":"user-1","text":"scheduled ping","interval_seconds":60,"create_response":false}"#,
            ))
            .expect("request");

        let (status, body) = request_json(app, request).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body["code"], serde_json::json!("auth_required"));
    }

    #[tokio::test]
    async fn test_channel_job_http_create_list_cancel_with_bearer_token() {
        let state = make_state(MockRunner::ok(), true);
        let token = paired_token(&state);
        let app = build_router(state);

        let create_request = Request::builder()
            .method("POST")
            .uri("/channels/jobs/create")
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::AUTHORIZATION, format!("Bearer {}", token))
            .body(Body::from(
                r#"{"channel":"webhook","account_id":"acct-job","external_user_id":"user-job","text":"scheduled ping","interval_seconds":3600,"create_response":false}"#,
            ))
            .expect("request");
        let (create_status, create_body) = request_json(app.clone(), create_request).await;
        assert_eq!(create_status, StatusCode::OK);
        assert_eq!(create_body["type"], serde_json::json!("ChannelJobCreated"));
        let job_id = create_body["job"]["job_id"]
            .as_str()
            .expect("job_id")
            .to_string();

        let list_request = Request::builder()
            .method("GET")
            .uri("/channels/jobs/list")
            .header(header::AUTHORIZATION, format!("Bearer {}", token))
            .body(Body::empty())
            .expect("request");
        let (list_status, list_body) = request_json(app.clone(), list_request).await;
        assert_eq!(list_status, StatusCode::OK);
        assert_eq!(list_body["type"], serde_json::json!("ChannelJobs"));
        let jobs = list_body["jobs"].as_array().expect("jobs array");
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0]["job_id"], serde_json::json!(job_id.clone()));

        let cancel_request = Request::builder()
            .method("POST")
            .uri("/channels/jobs/cancel")
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::AUTHORIZATION, format!("Bearer {}", token))
            .body(Body::from(format!(r#"{{"job_id":"{}"}}"#, job_id)))
            .expect("request");
        let (cancel_status, cancel_body) = request_json(app.clone(), cancel_request).await;
        assert_eq!(cancel_status, StatusCode::OK);
        assert_eq!(cancel_body["type"], serde_json::json!("ChannelJobCanceled"));

        let list_after_request = Request::builder()
            .method("GET")
            .uri("/channels/jobs/list")
            .header(header::AUTHORIZATION, format!("Bearer {}", token))
            .body(Body::empty())
            .expect("request");
        let (list_after_status, list_after_body) = request_json(app, list_after_request).await;
        assert_eq!(list_after_status, StatusCode::OK);
        assert_eq!(list_after_body["jobs"].as_array().map(|v| v.len()), Some(0));
    }

    #[tokio::test]
    async fn test_channel_job_http_create_rejects_zero_interval() {
        let state = make_state(MockRunner::ok(), true);
        let token = paired_token(&state);
        let app = build_router(state);

        let request = Request::builder()
            .method("POST")
            .uri("/channels/jobs/create")
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::AUTHORIZATION, format!("Bearer {}", token))
            .body(Body::from(
                r#"{"channel":"webhook","account_id":"acct-1","external_user_id":"user-1","text":"scheduled ping","interval_seconds":0,"create_response":false}"#,
            ))
            .expect("request");

        let (status, body) = request_json(app, request).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["code"], serde_json::json!("invalid_channel_job"));
    }

    #[tokio::test]
    async fn test_channel_webhook_supervised_action_http_requires_auth_token() {
        let state = make_state(MockRunner::ok(), true);
        let app = build_router(state);

        let request = Request::builder()
            .method("POST")
            .uri("/channels/webhook/supervised-action")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                r#"{"account_id":"acct-1","external_user_id":"user-1","tool_name":"echo_text","arguments":{"text":"hello"}}"#,
            ))
            .expect("request");

        let (status, body) = request_json(app, request).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body["code"], serde_json::json!("auth_required"));
    }

    #[tokio::test]
    async fn test_channel_webhook_supervised_action_http_returns_tool_result_and_uses_supervised_role(
    ) {
        let runner = MockRunner::ok();
        let create_configs = runner.created_session_configs.clone();
        let state = make_state(runner, true);
        let token = paired_token(&state);
        let app = build_router(state);

        let request = Request::builder()
            .method("POST")
            .uri("/channels/webhook/supervised-action")
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::AUTHORIZATION, format!("Bearer {}", token))
            .body(Body::from(
                r#"{
                  "account_id":"acct-1",
                  "external_user_id":"user-1",
                  "text":"trigger supervised action",
                  "tool_name":"echo_text",
                  "arguments":{"text":"hello from webhook action"}
                }"#,
            ))
            .expect("request");

        let (status, body) = request_json(app, request).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            body["type"],
            serde_json::json!("WebhookSupervisedActionResult")
        );
        assert_eq!(body["channel"], serde_json::json!("webhook"));
        assert_eq!(body["account_id"], serde_json::json!("acct-1"));
        assert_eq!(body["external_user_id"], serde_json::json!("user-1"));
        assert_eq!(body["tool_name"], serde_json::json!("echo_text"));
        assert_eq!(body["result"]["status"], serde_json::json!("ok"));
        assert!(body["session_id"].as_str().is_some());

        let configs = create_configs
            .lock()
            .expect("created_session_configs mutex poisoned");
        assert_eq!(configs.len(), 1);
        let cfg = configs[0].as_ref().expect("session config");
        assert_eq!(cfg.role.as_deref(), Some("supervised"));
        assert_eq!(cfg.enable_graph, Some(true));
    }

    // --- Base64 helpers ---

    #[test]
    fn test_base64_decode_valid() {
        // "AQID" = [1, 2, 3]
        let decoded = base64_decode("AQID").unwrap();
        assert_eq!(decoded, vec![1, 2, 3]);
    }

    #[test]
    fn test_base64_decode_with_padding() {
        // "YQ==" = "a"
        let decoded = base64_decode("YQ==").unwrap();
        assert_eq!(decoded, vec![b'a']);
    }

    #[test]
    fn test_base64_decode_empty() {
        let decoded = base64_decode("").unwrap();
        assert!(decoded.is_empty());
    }

    #[test]
    fn test_base64_decode_invalid() {
        assert!(base64_decode("!!!").is_none());
    }

    #[test]
    fn test_base64_encode_roundtrip() {
        let data = vec![1, 2, 3, 4, 5];
        let encoded = base64_encode(&data);
        let decoded = base64_decode(&encoded).unwrap();
        assert_eq!(decoded, data);
    }

    #[test]
    fn test_base64_encode_empty() {
        assert_eq!(base64_encode(&[]), "");
    }

    // --- GatewayConfig defaults ---

    #[test]
    fn test_gateway_config_defaults() {
        let config = GatewayConfig::default();
        assert_eq!(config.host, "127.0.0.1");
        assert_eq!(config.port, 8420);
    }

    // --- ConnectionState ---

    #[test]
    fn test_connection_state_starts_unauthenticated() {
        let conn = ConnectionState::new();
        assert!(!conn.authenticated);
        assert!(conn.token.is_none());
        assert!(conn.principal_id.is_none());
        assert!(conn.owned_sessions.is_empty());
    }

    // --- auth_required_error ---

    #[test]
    fn test_auth_required_error_format() {
        let resp = auth_required_error();
        match resp {
            GatewayResponse::Error { code, message } => {
                assert_eq!(code, "auth_required");
                assert!(!message.is_empty());
            }
            other => panic!("Expected Error, got {:?}", other),
        }
    }
}
