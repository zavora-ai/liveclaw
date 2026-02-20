//! GraphAgent construction — optional orchestration layer that wraps a
//! [`RealtimeAgent`] in a graph with conditional routing and human-in-the-loop
//! approval for the `"supervised"` role.

use std::sync::Arc;

use anyhow::Result;
use serde_json::{json, Value};

use adk_graph::{
    AgentNode, GraphAgent, MemoryCheckpointer, NodeContext, NodeOutput, State, END, START,
};
use adk_realtime::RealtimeAgent;

const PENDING_TOOL_CALLS_KEY: &str = "pending_tool_calls";
const TOOL_RESULTS_KEY: &str = "tool_results";
const TOOLS_EXECUTED_COUNT_KEY: &str = "tools_executed_count";

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
        .node_fn("tools", |ctx: NodeContext| async move {
            Ok(execute_pending_tool_calls(&ctx.state))
        })
        .edge(START, "agent")
        .conditional_edge(
            "agent",
            |state: &State| {
                if has_pending_tool_calls(state) {
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

fn has_pending_tool_calls(state: &State) -> bool {
    state
        .get(PENDING_TOOL_CALLS_KEY)
        .and_then(|v| v.as_array())
        .is_some_and(|calls| !calls.is_empty())
}

fn execute_pending_tool_calls(state: &State) -> NodeOutput {
    let pending_calls = state
        .get(PENDING_TOOL_CALLS_KEY)
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let results = pending_calls
        .iter()
        .map(tools_node_result_from_call)
        .collect::<Vec<_>>();

    NodeOutput::new()
        .with_update(PENDING_TOOL_CALLS_KEY, json!([]))
        .with_update(TOOL_RESULTS_KEY, Value::Array(results))
        .with_update(TOOLS_EXECUTED_COUNT_KEY, json!(pending_calls.len()))
}

fn tools_node_result_from_call(call: &Value) -> Value {
    let tool_name = call
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("unknown_tool");
    let arguments = call
        .get("arguments")
        .cloned()
        .or_else(|| call.get("args").cloned())
        .unwrap_or_else(|| json!({}));

    json!({
        "tool": tool_name,
        "status": "executed",
        "arguments": arguments
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use adk_graph::{ExecutionConfig, GraphAgent, GraphError, START};

    #[test]
    fn pending_tool_calls_detected_only_for_non_empty_arrays() {
        let mut state = State::new();
        assert!(!has_pending_tool_calls(&state));

        state.insert(PENDING_TOOL_CALLS_KEY.to_string(), json!([]));
        assert!(!has_pending_tool_calls(&state));

        state.insert(
            PENDING_TOOL_CALLS_KEY.to_string(),
            json!([{ "name": "echo_text", "arguments": {"text": "hello"} }]),
        );
        assert!(has_pending_tool_calls(&state));
    }

    #[test]
    fn tools_node_drains_pending_calls_into_results() {
        let mut state = State::new();
        state.insert(
            PENDING_TOOL_CALLS_KEY.to_string(),
            json!([
                { "name": "echo_text", "arguments": {"text": "hello"} },
                { "name": "add_numbers", "arguments": {"a": 1, "b": 2} }
            ]),
        );

        let output = execute_pending_tool_calls(&state);

        assert_eq!(output.updates.get(PENDING_TOOL_CALLS_KEY), Some(&json!([])));
        assert_eq!(
            output.updates.get(TOOLS_EXECUTED_COUNT_KEY),
            Some(&json!(2))
        );

        let results = output
            .updates
            .get(TOOL_RESULTS_KEY)
            .and_then(Value::as_array)
            .expect("tool_results should be an array");
        assert_eq!(results.len(), 2);
        assert_eq!(results[0]["tool"], json!("echo_text"));
        assert_eq!(results[0]["status"], json!("executed"));
        assert_eq!(results[1]["tool"], json!("add_numbers"));
    }

    #[test]
    fn tools_node_handles_missing_tool_fields() {
        let mut state = State::new();
        state.insert(PENDING_TOOL_CALLS_KEY.to_string(), json!([{}]));

        let output = execute_pending_tool_calls(&state);
        let results = output
            .updates
            .get(TOOL_RESULTS_KEY)
            .and_then(Value::as_array)
            .expect("tool_results should be an array");

        assert_eq!(results.len(), 1);
        assert_eq!(results[0]["tool"], json!("unknown_tool"));
        assert_eq!(results[0]["arguments"], json!({}));
    }

    #[tokio::test]
    async fn supervised_tools_node_interrupts_before_execution() {
        let graph = GraphAgent::builder("s4-supervised-test")
            .node_fn("agent", |_ctx| async {
                Ok(NodeOutput::new().with_update(
                    PENDING_TOOL_CALLS_KEY,
                    json!([{ "name": "echo_text", "arguments": { "text": "hi" } }]),
                ))
            })
            .node_fn("tools", |ctx: NodeContext| async move {
                Ok(execute_pending_tool_calls(&ctx.state))
            })
            .edge(START, "agent")
            .conditional_edge(
                "agent",
                |state: &State| {
                    if has_pending_tool_calls(state) {
                        "tools".to_string()
                    } else {
                        END.to_string()
                    }
                },
                [("tools", "tools"), ("__end__", END)],
            )
            .edge("tools", END)
            .interrupt_before(&["tools"])
            .build()
            .expect("graph should build");

        let result = graph
            .invoke(State::new(), ExecutionConfig::new("thread-supervised"))
            .await;
        assert!(matches!(result, Err(GraphError::Interrupted(_))));
    }

    #[tokio::test]
    async fn non_supervised_tools_node_executes_pending_calls() {
        let graph = GraphAgent::builder("s4-nonsupervised-test")
            .node_fn("agent", |_ctx| async {
                Ok(NodeOutput::new().with_update(
                    PENDING_TOOL_CALLS_KEY,
                    json!([{ "name": "echo_text", "arguments": { "text": "hi" } }]),
                ))
            })
            .node_fn("tools", |ctx: NodeContext| async move {
                Ok(execute_pending_tool_calls(&ctx.state))
            })
            .edge(START, "agent")
            .conditional_edge(
                "agent",
                |state: &State| {
                    if has_pending_tool_calls(state) {
                        "tools".to_string()
                    } else {
                        END.to_string()
                    }
                },
                [("tools", "tools"), ("__end__", END)],
            )
            .edge("tools", END)
            .build()
            .expect("graph should build");

        let result = graph
            .invoke(State::new(), ExecutionConfig::new("thread-full"))
            .await
            .expect("graph should execute");

        assert_eq!(
            result.get(TOOLS_EXECUTED_COUNT_KEY),
            Some(&json!(1)),
            "tools node should execute and record one call"
        );
        assert_eq!(result.get(PENDING_TOOL_CALLS_KEY), Some(&json!([])));
    }
}
