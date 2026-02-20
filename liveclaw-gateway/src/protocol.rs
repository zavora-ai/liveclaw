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
    AudioOutput {
        session_id: SessionId,
        audio: String,
    },
    TranscriptUpdate {
        session_id: SessionId,
        text: String,
        is_final: bool,
    },
    Diagnostics {
        data: RuntimeDiagnostics,
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
    pub active_sessions: usize,
}

/// Return supported client->gateway message type tags.
pub fn supported_client_message_types() -> Vec<String> {
    vec![
        "Pair".to_string(),
        "Authenticate".to_string(),
        "CreateSession".to_string(),
        "TerminateSession".to_string(),
        "SessionAudio".to_string(),
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
        "AudioOutput".to_string(),
        "TranscriptUpdate".to_string(),
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
            data: RuntimeDiagnostics {
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
                active_sessions: 1,
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
        assert!(server_types.contains(&"Diagnostics".to_string()));
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
