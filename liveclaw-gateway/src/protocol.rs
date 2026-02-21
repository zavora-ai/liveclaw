use serde::{Deserialize, Serialize};

/// Type alias for session identifiers.
pub type SessionId = String;
pub const PROTOCOL_VERSION: &str = "2026-02-20";

/// Messages sent from the client to the Gateway.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum GatewayMessage {
    Pair {
        code: String,
    },
    Authenticate {
        token: String,
    },
    CreateSession {
        config: Option<SessionConfig>,
    },
    TerminateSession {
        session_id: SessionId,
    },
    SessionAudio {
        session_id: SessionId,
        audio: String,
    },
    SessionAudioCommit {
        session_id: SessionId,
    },
    SessionResponseCreate {
        session_id: SessionId,
    },
    SessionResponseInterrupt {
        session_id: SessionId,
    },
    SessionToolCall {
        session_id: SessionId,
        tool_name: String,
        arguments: serde_json::Value,
    },
    PriorityProbe,
    GetGatewayHealth,
    GetDiagnostics,
    Ping,
}

/// Responses sent from the Gateway to the client.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum GatewayResponse {
    PairSuccess {
        token: String,
    },
    Authenticated {
        principal_id: String,
    },
    PairFailure {
        reason: String,
    },
    SessionCreated {
        session_id: SessionId,
    },
    SessionTerminated {
        session_id: SessionId,
    },
    AudioAccepted {
        session_id: SessionId,
    },
    AudioCommitted {
        session_id: SessionId,
    },
    ResponseCreateAccepted {
        session_id: SessionId,
    },
    ResponseInterruptAccepted {
        session_id: SessionId,
    },
    SessionToolResult {
        session_id: SessionId,
        tool_name: String,
        result: serde_json::Value,
        graph: GraphExecutionReport,
    },
    AudioOutput {
        session_id: SessionId,
        audio: String,
    },
    TranscriptUpdate {
        session_id: SessionId,
        text: String,
        is_final: bool,
    },
    PriorityProbeAccepted {
        queued_standard: bool,
        queued_priority: bool,
    },
    PriorityNotice {
        data: PriorityNotice,
    },
    GatewayHealth {
        data: GatewayHealth,
    },
    Diagnostics {
        data: Box<RuntimeDiagnostics>,
    },
    Error {
        code: String,
        message: String,
    },
    Pong,
}

/// Optional configuration for a new voice session.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionConfig {
    pub model: Option<String>,
    pub voice: Option<String>,
    pub instructions: Option<String>,
    /// Access role: "readonly", "supervised", or "full"
    pub role: Option<String>,
    /// Whether to wrap the RealtimeAgent in a GraphAgent
    pub enable_graph: Option<bool>,
}

/// Graph execution trace event for SessionToolCall runs.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GraphExecutionEvent {
    pub step: usize,
    pub node: String,
    pub action: String,
    pub detail: String,
}

/// Graph execution report returned for SessionToolCall.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GraphExecutionReport {
    pub thread_id: String,
    pub completed: bool,
    pub interrupted: bool,
    pub events: Vec<GraphExecutionEvent>,
    pub final_state: serde_json::Value,
}

/// Runtime and feature diagnostics for operator validation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RuntimeDiagnostics {
    pub protocol_version: String,
    pub supported_client_messages: Vec<String>,
    pub supported_server_responses: Vec<String>,
    pub runtime_kind: String,
    pub provider_profile: String,
    pub provider_kind: String,
    pub provider_model: String,
    pub provider_base_url: Option<String>,
    pub reconnect_enabled: bool,
    pub reconnect_max_attempts: u32,
    pub reconnect_attempts_total: u64,
    pub reconnect_successes_total: u64,
    pub reconnect_failures_total: u64,
    pub compaction_enabled: bool,
    pub compaction_max_events_threshold: usize,
    pub compactions_applied_total: u64,
    pub security_workspace_root: String,
    pub security_forbidden_tool_paths: Vec<String>,
    pub security_deny_by_default_principal_allowlist: bool,
    pub security_principal_allowlist_size: usize,
    pub security_allow_public_bind: bool,
    pub active_sessions: usize,
}

/// Gateway runtime health snapshot for operator release checks.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GatewayHealth {
    pub protocol_version: String,
    pub uptime_seconds: u64,
    pub require_pairing: bool,
    pub active_sessions: usize,
    pub active_ws_bindings: usize,
    pub active_priority_bindings: usize,
}

/// Priority control-plane notice routed over the gateway's priority channel.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PriorityNotice {
    pub level: String,
    pub code: String,
    pub message: String,
    pub session_id: Option<String>,
}

/// Return supported client->gateway message type tags.
pub fn supported_client_message_types() -> Vec<String> {
    vec![
        "Pair".to_string(),
        "Authenticate".to_string(),
        "CreateSession".to_string(),
        "TerminateSession".to_string(),
        "SessionAudio".to_string(),
        "SessionAudioCommit".to_string(),
        "SessionResponseCreate".to_string(),
        "SessionResponseInterrupt".to_string(),
        "SessionToolCall".to_string(),
        "PriorityProbe".to_string(),
        "GetGatewayHealth".to_string(),
        "GetDiagnostics".to_string(),
        "Ping".to_string(),
    ]
}

/// Return supported gateway->client response type tags.
pub fn supported_server_response_types() -> Vec<String> {
    vec![
        "PairSuccess".to_string(),
        "Authenticated".to_string(),
        "PairFailure".to_string(),
        "SessionCreated".to_string(),
        "SessionTerminated".to_string(),
        "AudioAccepted".to_string(),
        "AudioCommitted".to_string(),
        "ResponseCreateAccepted".to_string(),
        "ResponseInterruptAccepted".to_string(),
        "SessionToolResult".to_string(),
        "AudioOutput".to_string(),
        "TranscriptUpdate".to_string(),
        "PriorityProbeAccepted".to_string(),
        "PriorityNotice".to_string(),
        "GatewayHealth".to_string(),
        "Diagnostics".to_string(),
        "Error".to_string(),
        "Pong".to_string(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- GatewayMessage serialization tests ---

    #[test]
    fn test_gateway_message_pair_roundtrip() {
        let msg = GatewayMessage::Pair {
            code: "123456".into(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: GatewayMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(msg, parsed);
        assert!(json.contains(r#""type":"Pair"#));
    }

    #[test]
    fn test_gateway_message_create_session_with_config() {
        let msg = GatewayMessage::CreateSession {
            config: Some(SessionConfig {
                model: Some("gpt-4o-realtime".into()),
                voice: Some("alloy".into()),
                instructions: None,
                role: Some("supervised".into()),
                enable_graph: Some(true),
            }),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: GatewayMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(msg, parsed);
    }

    #[test]
    fn test_gateway_message_authenticate_roundtrip() {
        let msg = GatewayMessage::Authenticate {
            token: "tok-123".into(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: GatewayMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(msg, parsed);
    }

    #[test]
    fn test_gateway_message_create_session_no_config() {
        let msg = GatewayMessage::CreateSession { config: None };
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: GatewayMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(msg, parsed);
    }

    #[test]
    fn test_gateway_message_terminate_session() {
        let msg = GatewayMessage::TerminateSession {
            session_id: "sess-001".into(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: GatewayMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(msg, parsed);
    }

    #[test]
    fn test_gateway_message_session_audio() {
        let msg = GatewayMessage::SessionAudio {
            session_id: "sess-001".into(),
            audio: "base64encodedaudio==".into(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: GatewayMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(msg, parsed);
    }

    #[test]
    fn test_gateway_message_session_audio_commit() {
        let msg = GatewayMessage::SessionAudioCommit {
            session_id: "sess-001".into(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: GatewayMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(msg, parsed);
    }

    #[test]
    fn test_gateway_message_session_response_create() {
        let msg = GatewayMessage::SessionResponseCreate {
            session_id: "sess-001".into(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: GatewayMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(msg, parsed);
    }

    #[test]
    fn test_gateway_message_session_response_interrupt() {
        let msg = GatewayMessage::SessionResponseInterrupt {
            session_id: "sess-001".into(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: GatewayMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(msg, parsed);
    }

    #[test]
    fn test_gateway_message_session_tool_call() {
        let msg = GatewayMessage::SessionToolCall {
            session_id: "sess-001".into(),
            tool_name: "echo_text".into(),
            arguments: serde_json::json!({ "text": "hello" }),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: GatewayMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(msg, parsed);
    }

    #[test]
    fn test_gateway_message_ping() {
        let msg = GatewayMessage::Ping;
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: GatewayMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(msg, parsed);
        assert_eq!(json, r#"{"type":"Ping"}"#);
    }

    #[test]
    fn test_gateway_message_get_diagnostics() {
        let msg = GatewayMessage::GetDiagnostics;
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: GatewayMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(msg, parsed);
        assert_eq!(json, r#"{"type":"GetDiagnostics"}"#);
    }

    #[test]
    fn test_gateway_message_get_gateway_health() {
        let msg = GatewayMessage::GetGatewayHealth;
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: GatewayMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(msg, parsed);
        assert_eq!(json, r#"{"type":"GetGatewayHealth"}"#);
    }

    #[test]
    fn test_gateway_message_priority_probe() {
        let msg = GatewayMessage::PriorityProbe;
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: GatewayMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(msg, parsed);
        assert_eq!(json, r#"{"type":"PriorityProbe"}"#);
    }

    // --- GatewayResponse serialization tests ---

    #[test]
    fn test_gateway_response_pair_success() {
        let resp = GatewayResponse::PairSuccess {
            token: "tok-abc".into(),
        };
        let json = serde_json::to_string(&resp).unwrap();
        let parsed: GatewayResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(resp, parsed);
    }

    #[test]
    fn test_gateway_response_pair_failure() {
        let resp = GatewayResponse::PairFailure {
            reason: "invalid code".into(),
        };
        let json = serde_json::to_string(&resp).unwrap();
        let parsed: GatewayResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(resp, parsed);
    }

    #[test]
    fn test_gateway_response_authenticated() {
        let resp = GatewayResponse::Authenticated {
            principal_id: "principal-1".into(),
        };
        let json = serde_json::to_string(&resp).unwrap();
        let parsed: GatewayResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(resp, parsed);
    }

    #[test]
    fn test_gateway_response_session_created() {
        let resp = GatewayResponse::SessionCreated {
            session_id: "sess-002".into(),
        };
        let json = serde_json::to_string(&resp).unwrap();
        let parsed: GatewayResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(resp, parsed);
    }

    #[test]
    fn test_gateway_response_session_terminated() {
        let resp = GatewayResponse::SessionTerminated {
            session_id: "sess-002".into(),
        };
        let json = serde_json::to_string(&resp).unwrap();
        let parsed: GatewayResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(resp, parsed);
    }

    #[test]
    fn test_gateway_response_audio_accepted() {
        let resp = GatewayResponse::AudioAccepted {
            session_id: "sess-002".into(),
        };
        let json = serde_json::to_string(&resp).unwrap();
        let parsed: GatewayResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(resp, parsed);
    }

    #[test]
    fn test_gateway_response_audio_committed() {
        let resp = GatewayResponse::AudioCommitted {
            session_id: "sess-002".into(),
        };
        let json = serde_json::to_string(&resp).unwrap();
        let parsed: GatewayResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(resp, parsed);
    }

    #[test]
    fn test_gateway_response_response_create_accepted() {
        let resp = GatewayResponse::ResponseCreateAccepted {
            session_id: "sess-002".into(),
        };
        let json = serde_json::to_string(&resp).unwrap();
        let parsed: GatewayResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(resp, parsed);
    }

    #[test]
    fn test_gateway_response_response_interrupt_accepted() {
        let resp = GatewayResponse::ResponseInterruptAccepted {
            session_id: "sess-002".into(),
        };
        let json = serde_json::to_string(&resp).unwrap();
        let parsed: GatewayResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(resp, parsed);
    }

    #[test]
    fn test_gateway_response_session_tool_result() {
        let resp = GatewayResponse::SessionToolResult {
            session_id: "sess-002".into(),
            tool_name: "echo_text".into(),
            result: serde_json::json!({ "text": "hello", "length": 5 }),
            graph: GraphExecutionReport {
                thread_id: "sess-002-graph-1".into(),
                completed: true,
                interrupted: false,
                events: vec![GraphExecutionEvent {
                    step: 0,
                    node: "resolve_tool".into(),
                    action: "node_end".into(),
                    detail: "resolved one pending tool".into(),
                }],
                final_state: serde_json::json!({
                    "tools_executed_count": 1,
                }),
            },
        };
        let json = serde_json::to_string(&resp).unwrap();
        let parsed: GatewayResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(resp, parsed);
    }

    #[test]
    fn test_gateway_response_audio_output() {
        let resp = GatewayResponse::AudioOutput {
            session_id: "sess-002".into(),
            audio: "base64audio==".into(),
        };
        let json = serde_json::to_string(&resp).unwrap();
        let parsed: GatewayResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(resp, parsed);
    }

    #[test]
    fn test_gateway_response_transcript_update() {
        let resp = GatewayResponse::TranscriptUpdate {
            session_id: "sess-002".into(),
            text: "Hello world".into(),
            is_final: true,
        };
        let json = serde_json::to_string(&resp).unwrap();
        let parsed: GatewayResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(resp, parsed);
    }

    #[test]
    fn test_gateway_response_error() {
        let resp = GatewayResponse::Error {
            code: "auth_required".into(),
            message: "Not authenticated".into(),
        };
        let json = serde_json::to_string(&resp).unwrap();
        let parsed: GatewayResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(resp, parsed);
    }

    #[test]
    fn test_gateway_response_diagnostics() {
        let resp = GatewayResponse::Diagnostics {
            data: Box::new(RuntimeDiagnostics {
                protocol_version: PROTOCOL_VERSION.to_string(),
                supported_client_messages: supported_client_message_types(),
                supported_server_responses: supported_server_response_types(),
                runtime_kind: "native".into(),
                provider_profile: "legacy".into(),
                provider_kind: "openai".into(),
                provider_model: "gpt-4o-realtime-preview".into(),
                provider_base_url: Some("wss://api.openai.com/v1/realtime".into()),
                reconnect_enabled: true,
                reconnect_max_attempts: 5,
                reconnect_attempts_total: 2,
                reconnect_successes_total: 1,
                reconnect_failures_total: 1,
                compaction_enabled: true,
                compaction_max_events_threshold: 500,
                compactions_applied_total: 3,
                security_workspace_root: ".".into(),
                security_forbidden_tool_paths: vec![".git".into(), "target".into()],
                security_deny_by_default_principal_allowlist: false,
                security_principal_allowlist_size: 0,
                security_allow_public_bind: false,
                active_sessions: 1,
            }),
        };
        let json = serde_json::to_string(&resp).unwrap();
        let parsed: GatewayResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(resp, parsed);
    }

    #[test]
    fn test_gateway_response_gateway_health() {
        let resp = GatewayResponse::GatewayHealth {
            data: GatewayHealth {
                protocol_version: PROTOCOL_VERSION.to_string(),
                uptime_seconds: 12,
                require_pairing: true,
                active_sessions: 2,
                active_ws_bindings: 1,
                active_priority_bindings: 1,
            },
        };
        let json = serde_json::to_string(&resp).unwrap();
        let parsed: GatewayResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(resp, parsed);
    }

    #[test]
    fn test_gateway_response_priority_probe_accepted() {
        let resp = GatewayResponse::PriorityProbeAccepted {
            queued_standard: true,
            queued_priority: true,
        };
        let json = serde_json::to_string(&resp).unwrap();
        let parsed: GatewayResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(resp, parsed);
    }

    #[test]
    fn test_gateway_response_priority_notice() {
        let resp = GatewayResponse::PriorityNotice {
            data: PriorityNotice {
                level: "info".to_string(),
                code: "priority_probe".to_string(),
                message: "Priority channel is active".to_string(),
                session_id: Some("sess-1".to_string()),
            },
        };
        let json = serde_json::to_string(&resp).unwrap();
        let parsed: GatewayResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(resp, parsed);
    }

    #[test]
    fn test_supported_protocol_message_lists_include_diagnostics_path() {
        let client_types = supported_client_message_types();
        let server_types = supported_server_response_types();
        assert!(client_types.contains(&"GetDiagnostics".to_string()));
        assert!(client_types.contains(&"GetGatewayHealth".to_string()));
        assert!(client_types.contains(&"PriorityProbe".to_string()));
        assert!(client_types.contains(&"SessionAudioCommit".to_string()));
        assert!(client_types.contains(&"SessionResponseCreate".to_string()));
        assert!(client_types.contains(&"SessionResponseInterrupt".to_string()));
        assert!(client_types.contains(&"SessionToolCall".to_string()));
        assert!(server_types.contains(&"Diagnostics".to_string()));
        assert!(server_types.contains(&"GatewayHealth".to_string()));
        assert!(server_types.contains(&"PriorityProbeAccepted".to_string()));
        assert!(server_types.contains(&"PriorityNotice".to_string()));
        assert!(server_types.contains(&"AudioCommitted".to_string()));
        assert!(server_types.contains(&"ResponseCreateAccepted".to_string()));
        assert!(server_types.contains(&"ResponseInterruptAccepted".to_string()));
        assert!(server_types.contains(&"SessionToolResult".to_string()));
    }

    #[test]
    fn test_gateway_response_pong() {
        let resp = GatewayResponse::Pong;
        let json = serde_json::to_string(&resp).unwrap();
        let parsed: GatewayResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(resp, parsed);
        assert_eq!(json, r#"{"type":"Pong"}"#);
    }

    // --- SessionConfig tests ---

    #[test]
    fn test_session_config_all_fields() {
        let config = SessionConfig {
            model: Some("gpt-4o".into()),
            voice: Some("shimmer".into()),
            instructions: Some("Be helpful".into()),
            role: Some("full".into()),
            enable_graph: Some(false),
        };
        let json = serde_json::to_string(&config).unwrap();
        let parsed: SessionConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(config, parsed);
    }

    #[test]
    fn test_session_config_all_none() {
        let config = SessionConfig {
            model: None,
            voice: None,
            instructions: None,
            role: None,
            enable_graph: None,
        };
        let json = serde_json::to_string(&config).unwrap();
        let parsed: SessionConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(config, parsed);
    }

    // --- Edge case tests ---

    #[test]
    fn test_empty_string_fields() {
        let msg = GatewayMessage::Pair { code: "".into() };
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: GatewayMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(msg, parsed);
    }

    #[test]
    fn test_unicode_content() {
        let resp = GatewayResponse::TranscriptUpdate {
            session_id: "s1".into(),
            text: "こんにちは 🌍 café".into(),
            is_final: false,
        };
        let json = serde_json::to_string(&resp).unwrap();
        let parsed: GatewayResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(resp, parsed);
    }

    #[test]
    fn test_deserialize_from_raw_json() {
        let json = r#"{"type":"Pair","code":"999999"}"#;
        let msg: GatewayMessage = serde_json::from_str(json).unwrap();
        assert_eq!(
            msg,
            GatewayMessage::Pair {
                code: "999999".into()
            }
        );
    }

    #[test]
    fn test_unknown_type_tag_fails() {
        let json = r#"{"type":"Unknown","data":"foo"}"#;
        let result = serde_json::from_str::<GatewayMessage>(json);
        assert!(result.is_err());
    }
}
