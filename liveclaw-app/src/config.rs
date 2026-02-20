use serde::{Deserialize, Serialize};

// ── Default helper functions ────────────────────────────────────────────────

fn default_host() -> String {
    "127.0.0.1".to_string()
}

fn default_port() -> u16 {
    8420
}

fn default_true() -> bool {
    true
}

fn default_role() -> String {
    "supervised".to_string()
}

fn default_rate_limit() -> u32 {
    100
}

fn default_audit_path() -> String {
    "audit.jsonl".to_string()
}

fn default_memory_backend() -> String {
    "in_memory".to_string()
}

fn default_recall_limit() -> usize {
    10
}

fn default_recursion_limit() -> usize {
    25
}

fn default_max_events_threshold() -> usize {
    500
}

fn default_artifact_path() -> String {
    "./artifacts".to_string()
}

fn default_max_attempts() -> u32 {
    5
}

fn default_lockout_secs() -> u64 {
    300
}

fn default_reconnect_max_attempts() -> u32 {
    5
}

fn default_reconnect_initial_backoff_ms() -> u64 {
    250
}

fn default_reconnect_max_backoff_ms() -> u64 {
    4_000
}

fn default_runtime_docker_image() -> String {
    "zavoraai/liveclaw-runtime:latest".to_string()
}

fn default_provider_profile() -> String {
    "legacy".to_string()
}

// ── AudioFormat enum ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub enum AudioFormat {
    #[default]
    Pcm16_24kHz,
    Pcm16_16kHz,
}

// ── Sub-config structs ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GatewayConfig {
    #[serde(default = "default_host")]
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default = "default_true")]
    pub require_pairing: bool,
}

impl Default for GatewayConfig {
    fn default() -> Self {
        Self {
            host: default_host(),
            port: default_port(),
            require_pairing: default_true(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct VoiceConfig {
    #[serde(default)]
    pub provider: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default)]
    pub model: String,
    pub base_url: Option<String>,
    pub voice: Option<String>,
    pub instructions: Option<String>,
    #[serde(default)]
    pub audio_format: AudioFormat,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SecurityConfig {
    #[serde(default = "default_role")]
    pub default_role: String,
    #[serde(default)]
    pub tool_allowlist: Vec<String>,
    #[serde(default = "default_rate_limit")]
    pub rate_limit_per_session: u32,
    #[serde(default = "default_audit_path")]
    pub audit_log_path: String,
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            default_role: default_role(),
            tool_allowlist: Vec::new(),
            rate_limit_per_session: default_rate_limit(),
            audit_log_path: default_audit_path(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PluginConfig {
    #[serde(default = "default_true")]
    pub enable_pii_redaction: bool,
    #[serde(default = "default_true")]
    pub enable_memory_autosave: bool,
    #[serde(default)]
    pub custom_blocked_keywords: Vec<String>,
}

impl Default for PluginConfig {
    fn default() -> Self {
        Self {
            enable_pii_redaction: default_true(),
            enable_memory_autosave: default_true(),
            custom_blocked_keywords: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MemoryConfig {
    #[serde(default = "default_memory_backend")]
    pub backend: String,
    #[serde(default = "default_recall_limit")]
    pub recall_limit: usize,
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            backend: default_memory_backend(),
            recall_limit: default_recall_limit(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GraphConfig {
    #[serde(default)]
    pub enable_graph: bool,
    #[serde(default = "default_recursion_limit")]
    pub recursion_limit: usize,
}

impl Default for GraphConfig {
    fn default() -> Self {
        Self {
            enable_graph: false,
            recursion_limit: default_recursion_limit(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CompactionConfig {
    #[serde(default)]
    pub enable_compaction: bool,
    #[serde(default = "default_max_events_threshold")]
    pub max_events_threshold: usize,
}

impl Default for CompactionConfig {
    fn default() -> Self {
        Self {
            enable_compaction: false,
            max_events_threshold: default_max_events_threshold(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ArtifactConfig {
    #[serde(default)]
    pub enable_artifacts: bool,
    #[serde(default = "default_artifact_path")]
    pub storage_path: String,
}

impl Default for ArtifactConfig {
    fn default() -> Self {
        Self {
            enable_artifacts: false,
            storage_path: default_artifact_path(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PairingConfig {
    #[serde(default = "default_max_attempts")]
    pub max_attempts: u32,
    #[serde(default = "default_lockout_secs")]
    pub lockout_duration_secs: u64,
}

impl Default for PairingConfig {
    fn default() -> Self {
        Self {
            max_attempts: default_max_attempts(),
            lockout_duration_secs: default_lockout_secs(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "lowercase")]
pub enum RuntimeKind {
    #[default]
    Native,
    Docker,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RuntimeConfig {
    #[serde(default)]
    pub kind: RuntimeKind,
    #[serde(default = "default_runtime_docker_image")]
    pub docker_image: String,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            kind: RuntimeKind::Native,
            docker_image: default_runtime_docker_image(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct ProviderProfileConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub api_key: String,
    #[serde(default)]
    pub model: String,
    pub base_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProvidersConfig {
    #[serde(default = "default_provider_profile")]
    pub active_profile: String,
    #[serde(default)]
    pub openai: ProviderProfileConfig,
    #[serde(default)]
    pub openai_compatible: ProviderProfileConfig,
}

impl Default for ProvidersConfig {
    fn default() -> Self {
        Self {
            active_profile: default_provider_profile(),
            openai: ProviderProfileConfig::default(),
            openai_compatible: ProviderProfileConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ResilienceConfig {
    #[serde(default = "default_true")]
    pub enable_reconnect: bool,
    #[serde(default = "default_reconnect_max_attempts")]
    pub max_reconnect_attempts: u32,
    #[serde(default = "default_reconnect_initial_backoff_ms")]
    pub initial_backoff_ms: u64,
    #[serde(default = "default_reconnect_max_backoff_ms")]
    pub max_backoff_ms: u64,
}

impl Default for ResilienceConfig {
    fn default() -> Self {
        Self {
            enable_reconnect: default_true(),
            max_reconnect_attempts: default_reconnect_max_attempts(),
            initial_backoff_ms: default_reconnect_initial_backoff_ms(),
            max_backoff_ms: default_reconnect_max_backoff_ms(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct TelemetryConfig {
    #[serde(default)]
    pub otlp_enabled: bool,
}

// ── Top-level config ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct LiveClawConfig {
    #[serde(default)]
    pub gateway: GatewayConfig,
    #[serde(default)]
    pub voice: VoiceConfig,
    #[serde(default)]
    pub security: SecurityConfig,
    #[serde(default)]
    pub plugin: PluginConfig,
    #[serde(default)]
    pub memory: MemoryConfig,
    #[serde(default)]
    pub graph: GraphConfig,
    #[serde(default)]
    pub compaction: CompactionConfig,
    #[serde(default)]
    pub artifact: ArtifactConfig,
    #[serde(default)]
    pub pairing: PairingConfig,
    #[serde(default)]
    pub runtime: RuntimeConfig,
    #[serde(default)]
    pub providers: ProvidersConfig,
    #[serde(default)]
    pub resilience: ResilienceConfig,
    #[serde(default)]
    pub telemetry: TelemetryConfig,
}

impl LiveClawConfig {
    /// Load configuration from a TOML file at the given path.
    /// Missing fields use documented defaults. Unknown fields are silently ignored.
    pub fn load(path: &str) -> anyhow::Result<Self> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("Failed to read config file '{}': {}", path, e))?;
        let config: Self = toml::from_str(&content)
            .map_err(|e| anyhow::anyhow!("Failed to parse TOML config '{}': {}", path, e))?;
        Ok(config)
    }

    /// Parse configuration from a TOML string.
    pub fn from_toml(toml_str: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(toml_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config_values() {
        let cfg = LiveClawConfig::default();

        // Gateway defaults
        assert_eq!(cfg.gateway.host, "127.0.0.1");
        assert_eq!(cfg.gateway.port, 8420);
        assert!(cfg.gateway.require_pairing);

        // Voice defaults
        assert_eq!(cfg.voice.provider, "");
        assert_eq!(cfg.voice.api_key, "");
        assert_eq!(cfg.voice.audio_format, AudioFormat::Pcm16_24kHz);
        assert_eq!(cfg.voice.base_url, None);
        assert_eq!(cfg.voice.voice, None);
        assert_eq!(cfg.voice.instructions, None);

        // Security defaults
        assert_eq!(cfg.security.default_role, "supervised");
        assert!(cfg.security.tool_allowlist.is_empty());
        assert_eq!(cfg.security.rate_limit_per_session, 100);
        assert_eq!(cfg.security.audit_log_path, "audit.jsonl");

        // Plugin defaults
        assert!(cfg.plugin.enable_pii_redaction);
        assert!(cfg.plugin.enable_memory_autosave);
        assert!(cfg.plugin.custom_blocked_keywords.is_empty());

        // Memory defaults
        assert_eq!(cfg.memory.backend, "in_memory");
        assert_eq!(cfg.memory.recall_limit, 10);

        // Graph defaults
        assert!(!cfg.graph.enable_graph);
        assert_eq!(cfg.graph.recursion_limit, 25);

        // Compaction defaults
        assert!(!cfg.compaction.enable_compaction);
        assert_eq!(cfg.compaction.max_events_threshold, 500);

        // Artifact defaults
        assert!(!cfg.artifact.enable_artifacts);
        assert_eq!(cfg.artifact.storage_path, "./artifacts");

        // Pairing defaults
        assert_eq!(cfg.pairing.max_attempts, 5);
        assert_eq!(cfg.pairing.lockout_duration_secs, 300);

        // Runtime/provider defaults
        assert_eq!(cfg.runtime.kind, RuntimeKind::Native);
        assert_eq!(cfg.runtime.docker_image, "zavoraai/liveclaw-runtime:latest");
        assert_eq!(cfg.providers.active_profile, "legacy");
        assert!(!cfg.providers.openai.enabled);
        assert!(!cfg.providers.openai_compatible.enabled);

        // Resilience defaults
        assert!(cfg.resilience.enable_reconnect);
        assert_eq!(cfg.resilience.max_reconnect_attempts, 5);
        assert_eq!(cfg.resilience.initial_backoff_ms, 250);
        assert_eq!(cfg.resilience.max_backoff_ms, 4_000);

        // Telemetry defaults
        assert!(!cfg.telemetry.otlp_enabled);
    }

    #[test]
    fn test_toml_round_trip() {
        let cfg = LiveClawConfig {
            gateway: GatewayConfig {
                host: "0.0.0.0".to_string(),
                port: 9000,
                require_pairing: false,
            },
            voice: VoiceConfig {
                provider: "openai".to_string(),
                api_key: "sk-test".to_string(),
                model: "gpt-4o".to_string(),
                base_url: Some("wss://api.openai.com/v1/realtime".to_string()),
                voice: Some("nova".to_string()),
                instructions: Some("Be helpful.".to_string()),
                audio_format: AudioFormat::Pcm16_16kHz,
            },
            security: SecurityConfig {
                default_role: "full".to_string(),
                tool_allowlist: vec!["read_file".to_string(), "search".to_string()],
                rate_limit_per_session: 50,
                audit_log_path: "/tmp/audit.jsonl".to_string(),
            },
            plugin: PluginConfig {
                enable_pii_redaction: false,
                enable_memory_autosave: false,
                custom_blocked_keywords: vec!["bad".to_string()],
            },
            memory: MemoryConfig {
                backend: "redis".to_string(),
                recall_limit: 20,
            },
            graph: GraphConfig {
                enable_graph: true,
                recursion_limit: 50,
            },
            compaction: CompactionConfig {
                enable_compaction: true,
                max_events_threshold: 1000,
            },
            artifact: ArtifactConfig {
                enable_artifacts: true,
                storage_path: "/data/artifacts".to_string(),
            },
            pairing: PairingConfig {
                max_attempts: 3,
                lockout_duration_secs: 600,
            },
            runtime: RuntimeConfig {
                kind: RuntimeKind::Docker,
                docker_image: "local/liveclaw-runtime:test".to_string(),
            },
            providers: ProvidersConfig {
                active_profile: "openai_compatible".to_string(),
                openai: ProviderProfileConfig {
                    enabled: true,
                    api_key: "sk-openai".to_string(),
                    model: "gpt-4o-realtime-preview".to_string(),
                    base_url: None,
                },
                openai_compatible: ProviderProfileConfig {
                    enabled: true,
                    api_key: "compat-key".to_string(),
                    model: "gpt-4o-realtime-preview".to_string(),
                    base_url: Some("wss://example.com/realtime".to_string()),
                },
            },
            resilience: ResilienceConfig {
                enable_reconnect: false,
                max_reconnect_attempts: 7,
                initial_backoff_ms: 100,
                max_backoff_ms: 2_000,
            },
            telemetry: TelemetryConfig { otlp_enabled: true },
        };

        let toml_str = toml::to_string(&cfg).expect("serialize");
        let deserialized: LiveClawConfig = toml::from_str(&toml_str).expect("deserialize");
        assert_eq!(cfg, deserialized);
    }

    #[test]
    fn test_partial_toml_uses_defaults() {
        let toml_str = r#"
[gateway]
port = 9999
"#;
        let cfg = LiveClawConfig::from_toml(toml_str).expect("parse partial");

        // Overridden value
        assert_eq!(cfg.gateway.port, 9999);

        // Defaults for everything else
        assert_eq!(cfg.gateway.host, "127.0.0.1");
        assert!(cfg.gateway.require_pairing);
        assert_eq!(cfg.security.default_role, "supervised");
        assert_eq!(cfg.memory.recall_limit, 10);
        assert!(!cfg.graph.enable_graph);
        assert_eq!(cfg.pairing.max_attempts, 5);
        assert_eq!(cfg.runtime.kind, RuntimeKind::Native);
        assert_eq!(cfg.providers.active_profile, "legacy");
        assert!(cfg.resilience.enable_reconnect);
        assert!(!cfg.telemetry.otlp_enabled);
    }

    #[test]
    fn test_empty_toml_uses_all_defaults() {
        let cfg = LiveClawConfig::from_toml("").expect("parse empty");
        assert_eq!(cfg, LiveClawConfig::default());
    }

    #[test]
    fn test_invalid_toml_returns_error() {
        let bad_toml = "this is not [valid toml }{";
        let result = LiveClawConfig::from_toml(bad_toml);
        assert!(result.is_err());
    }

    #[test]
    fn test_unknown_fields_ignored() {
        let toml_str = r#"
[gateway]
host = "10.0.0.1"
port = 8420
unknown_field = "should be ignored"

[some_unknown_section]
foo = "bar"
"#;
        let cfg = LiveClawConfig::from_toml(toml_str).expect("parse with unknown fields");
        assert_eq!(cfg.gateway.host, "10.0.0.1");
        assert_eq!(cfg.gateway.port, 8420);
    }

    #[test]
    fn test_load_dev_config_file() {
        // Try multiple paths since test CWD varies
        let cfg = LiveClawConfig::load("dev/liveclaw.dev.toml")
            .or_else(|_| LiveClawConfig::load("liveclaw-app/dev/liveclaw.dev.toml"))
            .or_else(|_| LiveClawConfig::load("liveclaw/dev/liveclaw.dev.toml"))
            .or_else(|_| LiveClawConfig::load("liveclaw/liveclaw-app/dev/liveclaw.dev.toml"))
            .or_else(|_| LiveClawConfig::load("../dev/liveclaw.dev.toml"));

        // Skip test if dev config not found (CI environments)
        let cfg = match cfg {
            Ok(c) => c,
            Err(_) => return,
        };

        assert_eq!(cfg.gateway.host, "127.0.0.1");
        assert_eq!(cfg.gateway.port, 8420);
        assert!(!cfg.gateway.require_pairing); // dev config disables pairing
        assert_eq!(cfg.voice.provider, "openai");
        assert_eq!(cfg.voice.model, "gpt-4o-realtime-preview");
        assert_eq!(cfg.security.default_role, "full"); // dev config uses full
        assert_eq!(cfg.security.rate_limit_per_session, 1000);
        assert_eq!(cfg.memory.backend, "in_memory");
        assert_eq!(cfg.memory.recall_limit, 10);
        assert!(!cfg.graph.enable_graph);
        assert_eq!(cfg.graph.recursion_limit, 25);
        assert!(!cfg.compaction.enable_compaction);
        assert_eq!(cfg.compaction.max_events_threshold, 500);
        assert!(!cfg.artifact.enable_artifacts);
        assert_eq!(cfg.pairing.max_attempts, 5);
        assert_eq!(cfg.runtime.kind, RuntimeKind::Native);
        assert_eq!(cfg.providers.active_profile, "legacy");
        assert!(cfg.resilience.enable_reconnect);
        assert_eq!(cfg.resilience.max_reconnect_attempts, 5);
        assert_eq!(cfg.resilience.initial_backoff_ms, 250);
        assert_eq!(cfg.resilience.max_backoff_ms, 4_000);
        assert!(!cfg.telemetry.otlp_enabled);
    }

    #[test]
    fn test_load_nonexistent_file_returns_error() {
        let result = LiveClawConfig::load("/nonexistent/path/config.toml");
        assert!(result.is_err());
    }

    #[test]
    fn test_audio_format_default() {
        assert_eq!(AudioFormat::default(), AudioFormat::Pcm16_24kHz);
    }

    #[test]
    fn test_audio_format_round_trip() {
        // TOML requires a table at the top level, so wrap in a struct
        #[derive(Debug, Serialize, Deserialize, PartialEq)]
        struct Wrapper {
            fmt: AudioFormat,
        }

        for fmt in [AudioFormat::Pcm16_24kHz, AudioFormat::Pcm16_16kHz] {
            let w = Wrapper { fmt: fmt.clone() };
            let toml_str = toml::to_string(&w).expect("serialize audio format");
            let deserialized: Wrapper =
                toml::from_str(&toml_str).expect("deserialize audio format");
            assert_eq!(w, deserialized);
        }
    }

    #[test]
    fn test_runtime_kind_rejects_unknown_value() {
        let toml_str = r#"
[runtime]
kind = "invalid"
"#;
        let result = LiveClawConfig::from_toml(toml_str);
        assert!(result.is_err());
    }
}
