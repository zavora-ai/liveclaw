use serde::{Deserialize, Serialize};

/// Type alias for session identifiers.
pub type SessionId = String;

/// Messages sent from the client to the Gateway.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum GatewayMessage {
    Pair {
        code: String,
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
    Ping,
}

/// Responses sent from the Gateway to the client.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum GatewayResponse {
    PairSuccess {
        token: String,
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
