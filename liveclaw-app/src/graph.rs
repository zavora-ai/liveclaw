//! GraphAgent construction — optional orchestration layer that wraps a
//! [`RealtimeAgent`] in a graph with conditional routing and human-in-the-loop
//! approval for the `"supervised"` role.

use std::sync::Arc;

use anyhow::Result;

use adk_graph::{AgentNode, GraphAgent, MemoryCheckpointer, NodeContext, NodeOutput, State, START, END};
use adk_realtime::RealtimeAgent;

// ---------------------------------------------------------------------------
// GraphConfig (minimal stub until config.rs Task 11.1)
// ---------------------------------------------------------------------------

/// Graph orchestration settings.
///
/// A full version with serde derives will live in `config.rs` once Task 11.1
/// lands. This struct carries just enough to wire up the GraphAgent.
pub struct GraphConfig {
    /// Whether graph orchestration is enabled.
    pub enable_graph: bool,
    /// Maximum recursion depth for cyclic graphs (default 25).
    pub recursion_limit: Option<usize>,
}

// ---------------------------------------------------------------------------
// GraphAgent construction
// ---------------------------------------------------------------------------

/// Build a [`GraphAgent`] that wraps the given [`RealtimeAgent`] for
/// multi-step orchestration with a ReAct-style tool loop.
///
/// The graph has two nodes:
/// - **agent** — the `RealtimeAgent` wrapped in an [`AgentNode`].
/// - **tools** — executes any pending tool calls produced by the agent.
///
/// Edges:
/// - `START → agent`
/// - `agent → tools | end` (conditional on pending tool calls)
/// - `tools → agent` (cycle back for the ReAct pattern)
///
/// When `role` is `"supervised"`, [`interrupt_before`] is set on the
/// `"tools"` node so a human can approve tool calls before execution.
pub fn build_graph_agent(
    realtime_agent: RealtimeAgent,
    config: &GraphConfig,
    role: &str,
) -> Result<GraphAgent> {
    let agent_node = AgentNode::new(Arc::new(realtime_agent));

    let mut builder = GraphAgent::builder("voice_orchestrator")
        .node(agent_node)
        .node_fn("tools", |_ctx: NodeContext| async move {
            // Tool execution node — in a full implementation this would
            // iterate pending tool calls from the state and execute them.
            Ok(NodeOutput::new())
        })
        .edge(START, "agent")
        .conditional_edge(
            "agent",
            |state: &State| {
                if state.get("pending_tool_calls").and_then(|v| v.as_array()).map_or(false, |a| !a.is_empty()) {
                    "tools".to_string()
                } else {
                    END.to_string()
                }
            },
            [("tools", "tools"), ("__end__", END)],
        )
        .edge("tools", "agent")
        .checkpointer(MemoryCheckpointer::new())
        .recursion_limit(config.recursion_limit.unwrap_or(25));

    // Human-in-the-loop for supervised role
    if role == "supervised" {
        builder = builder.interrupt_before(&["tools"]);
    }

    builder.build().map_err(|e| anyhow::anyhow!("{}", e))
}


