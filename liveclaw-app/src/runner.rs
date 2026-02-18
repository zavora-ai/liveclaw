//! Runner construction — wires together the agent, session service, memory,
//! artifacts, plugins, and compaction config into an `adk-runner::Runner`.

use std::sync::Arc;

use adk_artifact::ArtifactService;
use adk_core::{Agent, Memory};
use adk_plugin::PluginManager;
use adk_runner::{EventsCompactionConfig, Runner, RunnerConfig};
use adk_session::SessionService;
use anyhow::Result;

use crate::memory::MemoryAdapter;

/// Build the [`Runner`] that manages the full session lifecycle.
///
/// The runner handles session get/create, memory injection into
/// `InvocationContext`, plugin lifecycle hooks (`before_run`, `after_run`,
/// `on_event`), event persistence via `SessionService::append_event()`, and
/// optional context compaction for long-running voice sessions.
///
/// # Arguments
///
/// * `agent` — a `RealtimeAgent` or `GraphAgent` wrapping one
/// * `session_service` — event-sourced session storage (e.g. `InMemorySessionService`)
/// * `memory_adapter` — optional bridge from `MemoryStore` to `adk-core::Memory`
/// * `artifact_service` — optional artifact storage for audio/transcripts
/// * `plugin_manager` — optional plugin manager (PII redaction, guardrails, etc.)
/// * `compaction_config` — optional event compaction settings
pub fn build_runner(
    agent: Arc<dyn Agent>,
    session_service: Arc<dyn SessionService>,
    memory_adapter: Option<Arc<MemoryAdapter>>,
    artifact_service: Option<Arc<dyn ArtifactService>>,
    plugin_manager: Option<Arc<PluginManager>>,
    compaction_config: Option<EventsCompactionConfig>,
) -> Result<Runner> {
    Ok(Runner::new(RunnerConfig {
        app_name: "liveclaw".to_string(),
        agent,
        session_service,
        artifact_service,
        memory_service: memory_adapter.map(|m| m as Arc<dyn Memory>),
        plugin_manager,
        run_config: None,
        compaction_config,
    })?)
}
