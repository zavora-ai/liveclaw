use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use axum::extract::ws::{Message, WebSocket};
use axum::extract::{State, WebSocketUpgrade};
use axum::response::{IntoResponse, Json};
use axum::routing::get;
use axum::Router;
use serde::Serialize;
use tokio::net::TcpListener;
use tokio::sync::{mpsc, RwLock};
use tracing::{error, info, warn};

use crate::pairing::PairingGuard;
use crate::protocol::{GatewayMessage, GatewayResponse, SessionConfig};

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
    audio_senders: Arc<RwLock<HashMap<String, mpsc::Sender<Vec<u8>>>>>,
    /// Per-session senders for forwarding GatewayResponse messages (AudioOutput,
    /// TranscriptUpdate) back to the WebSocket connection that owns the session.
    ws_response_senders: Arc<RwLock<HashMap<String, mpsc::Sender<GatewayResponse>>>>,
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

        let state = AppState {
            pairing: self.pairing,
            runner: self.runner,
            audio_senders: self.audio_senders,
            ws_response_senders: ws_response_senders.clone(),
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
            tokio::spawn(async move {
                while let Some(output) = transcript_rx.recv().await {
                    let resp = GatewayResponse::TranscriptUpdate {
                        session_id: output.session_id.clone(),
                        text: output.text,
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
                }
                info!("Transcript output channel closed");
            });
        }

        let app = Router::new()
            .route("/health", get(health_handler))
            .route("/ws", get(ws_upgrade_handler))
            .with_state(state);

        let addr = format!("{}:{}", self.config.host, self.config.port);
        info!("Gateway listening on {}", addr);

        let listener = TcpListener::bind(&addr).await?;
        axum::serve(listener, app).await?;

        Ok(())
    }
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

// ---------------------------------------------------------------------------
// WebSocket connection handler
// ---------------------------------------------------------------------------

/// Per-connection state tracking authentication and owned sessions.
struct ConnectionState {
    authenticated: bool,
    token: Option<String>,
    user_id: Option<String>,
    /// Session IDs owned by this connection, used to register/unregister
    /// ws_response_senders for audio/transcript forwarding.
    owned_sessions: Vec<String>,
}

impl ConnectionState {
    fn new() -> Self {
        Self {
            authenticated: false,
            token: None,
            user_id: None,
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
        conn.user_id = Some("anonymous".to_string());
    }

    // Per-connection channel for receiving server-pushed responses
    let (ws_tx, mut ws_rx) = mpsc::channel::<GatewayResponse>(256);

    loop {
        tokio::select! {
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

                let resp = handle_message(gateway_msg, &state, &mut conn, &ws_tx).await;
                if send_response(&mut ws, &resp).await.is_err() {
                    break;
                }
            }

            // Server → Client: forwarded AudioOutput / TranscriptUpdate responses
            Some(resp) = ws_rx.recv() => {
                if send_response(&mut ws, &resp).await.is_err() {
                    break;
                }
            }
        }
    }

    // Cleanup: remove ws_response_senders for all sessions owned by this connection
    {
        let mut senders = state.ws_response_senders.write().await;
        for session_id in &conn.owned_sessions {
            senders.remove(session_id);
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
) -> GatewayResponse {
    match msg {
        GatewayMessage::Ping => GatewayResponse::Pong,

        GatewayMessage::Pair { code } => handle_pair(&code, state, conn),

        GatewayMessage::CreateSession { config } => {
            if !conn.authenticated {
                return auth_required_error();
            }
            handle_create_session(config, state, conn, ws_tx).await
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
            handle_session_audio(&session_id, &audio, state).await
        }
    }
}

/// Construct the standard auth_required error response.
fn auth_required_error() -> GatewayResponse {
    GatewayResponse::Error {
        code: "auth_required".to_string(),
        message: "Authentication required. Send a Pair message first.".to_string(),
    }
}

// ---------------------------------------------------------------------------
// Individual message handlers
// ---------------------------------------------------------------------------

/// Handle a Pair message — delegates to PairingGuard.
fn handle_pair(code: &str, state: &AppState, conn: &mut ConnectionState) -> GatewayResponse {
    match state.pairing.try_pair(code) {
        Ok(Some(token)) => {
            conn.authenticated = true;
            conn.token = Some(token.clone());
            conn.user_id = Some(uuid::Uuid::new_v4().to_string());
            GatewayResponse::PairSuccess { token }
        }
        Ok(None) => {
            // Pairing disabled — auto-authenticated
            conn.authenticated = true;
            conn.user_id = Some("anonymous".to_string());
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

/// Handle a CreateSession message — delegates to Runner and registers the
/// session's ws_response_sender so audio/transcript output can be forwarded.
async fn handle_create_session(
    config: Option<SessionConfig>,
    state: &AppState,
    conn: &mut ConnectionState,
    ws_tx: &mpsc::Sender<GatewayResponse>,
) -> GatewayResponse {
    let user_id = conn.user_id.as_deref().unwrap_or("unknown");
    let session_id = uuid::Uuid::new_v4().to_string();

    match state
        .runner
        .create_session(user_id, &session_id, config)
        .await
    {
        Ok(sid) => {
            // Register the ws_response_sender so background tasks can forward
            // AudioOutput and TranscriptUpdate to this connection.
            state
                .ws_response_senders
                .write()
                .await
                .insert(sid.clone(), ws_tx.clone());
            conn.owned_sessions.push(sid.clone());
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
    // Remove audio sender for this session
    state.audio_senders.write().await.remove(session_id);
    // Remove ws_response_sender for this session
    state.ws_response_senders.write().await.remove(session_id);
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
) -> GatewayResponse {
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
            // Audio forwarded successfully — no explicit response needed per protocol,
            // but we acknowledge to keep the client informed.
            GatewayResponse::SessionCreated {
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

    /// A mock RunnerHandle for testing message routing logic.
    struct MockRunner {
        /// If set, create_session returns this error.
        create_err: Option<String>,
        /// If set, terminate_session returns this error.
        terminate_err: Option<String>,
        /// If set, send_audio returns this error.
        audio_err: Option<String>,
    }

    impl MockRunner {
        fn ok() -> Self {
            Self {
                create_err: None,
                terminate_err: None,
                audio_err: None,
            }
        }

        fn with_create_err(msg: &str) -> Self {
            Self {
                create_err: Some(msg.to_string()),
                terminate_err: None,
                audio_err: None,
            }
        }
    }

    #[async_trait]
    impl RunnerHandle for MockRunner {
        async fn create_session(
            &self,
            _user_id: &str,
            session_id: &str,
            _config: Option<SessionConfig>,
        ) -> anyhow::Result<String> {
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
    }

    fn make_state(runner: MockRunner, require_pairing: bool) -> AppState {
        AppState {
            pairing: Arc::new(PairingGuard::new(require_pairing, &[])),
            runner: Arc::new(runner),
            audio_senders: Arc::new(RwLock::new(HashMap::new())),
            ws_response_senders: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    fn authed_conn() -> ConnectionState {
        ConnectionState {
            authenticated: true,
            token: Some("test-token".to_string()),
            user_id: Some("user-1".to_string()),
            owned_sessions: Vec::new(),
        }
    }

    fn unauthed_conn() -> ConnectionState {
        ConnectionState::new()
    }

    /// Create a dummy ws_tx for tests that don't need to inspect forwarded responses.
    fn dummy_ws_tx() -> mpsc::Sender<GatewayResponse> {
        let (tx, _rx) = mpsc::channel(16);
        tx
    }

    // --- Ping/Pong ---

    #[tokio::test]
    async fn test_ping_returns_pong() {
        let state = make_state(MockRunner::ok(), true);
        let mut conn = unauthed_conn();
        let ws_tx = dummy_ws_tx();
        let resp = handle_message(GatewayMessage::Ping, &state, &mut conn, &ws_tx).await;
        assert_eq!(resp, GatewayResponse::Pong);
    }

    #[tokio::test]
    async fn test_ping_works_without_auth() {
        let state = make_state(MockRunner::ok(), true);
        let mut conn = unauthed_conn();
        let ws_tx = dummy_ws_tx();
        assert!(!conn.authenticated);
        let resp = handle_message(GatewayMessage::Ping, &state, &mut conn, &ws_tx).await;
        assert_eq!(resp, GatewayResponse::Pong);
    }

    // --- Auth required for session commands ---

    #[tokio::test]
    async fn test_create_session_requires_auth() {
        let state = make_state(MockRunner::ok(), true);
        let mut conn = unauthed_conn();
        let ws_tx = dummy_ws_tx();
        let resp = handle_message(
            GatewayMessage::CreateSession { config: None },
            &state,
            &mut conn,
            &ws_tx,
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
        let ws_tx = dummy_ws_tx();
        let resp = handle_message(
            GatewayMessage::TerminateSession {
                session_id: "s1".into(),
            },
            &state,
            &mut conn,
            &ws_tx,
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
        let ws_tx = dummy_ws_tx();
        let resp = handle_message(
            GatewayMessage::SessionAudio {
                session_id: "s1".into(),
                audio: "AAAA".into(),
            },
            &state,
            &mut conn,
            &ws_tx,
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
        let ws_tx = dummy_ws_tx();
        let code = state.pairing.pairing_code().unwrap();

        let resp = handle_message(GatewayMessage::Pair { code }, &state, &mut conn, &ws_tx).await;
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
        let ws_tx = dummy_ws_tx();
        let code = state.pairing.pairing_code().unwrap();
        let wrong = if code == "999999" { "000000" } else { "999999" };

        let resp = handle_message(
            GatewayMessage::Pair {
                code: wrong.to_string(),
            },
            &state,
            &mut conn,
            &ws_tx,
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
        let ws_tx = dummy_ws_tx();

        let resp = handle_message(
            GatewayMessage::Pair {
                code: "anything".into(),
            },
            &state,
            &mut conn,
            &ws_tx,
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

    // --- Session creation ---

    #[tokio::test]
    async fn test_create_session_success() {
        let state = make_state(MockRunner::ok(), false);
        let mut conn = authed_conn();
        let ws_tx = dummy_ws_tx();

        let resp = handle_message(
            GatewayMessage::CreateSession { config: None },
            &state,
            &mut conn,
            &ws_tx,
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
        let ws_tx = dummy_ws_tx();
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
        let ws_tx = dummy_ws_tx();

        let resp = handle_message(
            GatewayMessage::CreateSession { config: None },
            &state,
            &mut conn,
            &ws_tx,
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
        let ws_tx = dummy_ws_tx();

        let resp = handle_message(
            GatewayMessage::TerminateSession {
                session_id: "sess-1".into(),
            },
            &state,
            &mut conn,
            &ws_tx,
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
        let ws_tx = dummy_ws_tx();

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
        conn.owned_sessions.push("sess-1".to_string());

        let resp = handle_message(
            GatewayMessage::TerminateSession {
                session_id: "sess-1".into(),
            },
            &state,
            &mut conn,
            &ws_tx,
        )
        .await;
        assert!(matches!(resp, GatewayResponse::SessionTerminated { .. }));
        assert!(!state.audio_senders.read().await.contains_key("sess-1"));
        assert!(!state
            .ws_response_senders
            .read()
            .await
            .contains_key("sess-1"));
        assert!(!conn.owned_sessions.contains(&"sess-1".to_string()));
    }

    // --- Session audio ---

    #[tokio::test]
    async fn test_session_audio_forwards_to_runner() {
        let state = make_state(MockRunner::ok(), false);
        let mut conn = authed_conn();
        let ws_tx = dummy_ws_tx();

        let resp = handle_message(
            GatewayMessage::SessionAudio {
                session_id: "sess-1".into(),
                audio: "AQID".into(), // base64 for [1, 2, 3]
            },
            &state,
            &mut conn,
            &ws_tx,
        )
        .await;
        // Should not be an error
        if let GatewayResponse::Error { .. } = &resp {
            panic!("Expected success, got {:?}", resp)
        }
    }

    #[tokio::test]
    async fn test_session_audio_invalid_base64() {
        let state = make_state(MockRunner::ok(), false);
        let mut conn = authed_conn();
        let ws_tx = dummy_ws_tx();

        let resp = handle_message(
            GatewayMessage::SessionAudio {
                session_id: "sess-1".into(),
                audio: "!!!invalid!!!".into(),
            },
            &state,
            &mut conn,
            &ws_tx,
        )
        .await;
        match resp {
            GatewayResponse::Error { code, .. } => assert_eq!(code, "invalid_audio"),
            other => panic!("Expected invalid_audio error, got {:?}", other),
        }
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
        assert!(conn.user_id.is_none());
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
