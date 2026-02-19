use std::sync::Arc;

use adk_core::{Tool, ToolContext};
use adk_tool::FunctionTool;
use chrono::Utc;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
struct EchoArgs {
    #[serde(default)]
    text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
struct EchoResponse {
    text: String,
    length: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
struct AddArgs {
    #[serde(default)]
    a: f64,
    #[serde(default)]
    b: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
struct AddResponse {
    sum: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
struct UtcTimeArgs {}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
struct UtcTimeResponse {
    utc_rfc3339: String,
    unix_seconds: i64,
}

fn echo_tool() -> Arc<dyn Tool> {
    Arc::new(
        FunctionTool::new(
            "echo_text",
            "Echoes input text and returns basic length metadata.",
            |_ctx: Arc<dyn ToolContext>, args| async move {
                let parsed = serde_json::from_value::<EchoArgs>(args).unwrap_or_default();
                Ok(json!(EchoResponse {
                    length: parsed.text.chars().count(),
                    text: parsed.text,
                }))
            },
        )
        .with_parameters_schema::<EchoArgs>()
        .with_response_schema::<EchoResponse>(),
    )
}

fn add_tool() -> Arc<dyn Tool> {
    Arc::new(
        FunctionTool::new(
            "add_numbers",
            "Adds two numeric arguments and returns the sum.",
            |_ctx: Arc<dyn ToolContext>, args| async move {
                let parsed = serde_json::from_value::<AddArgs>(args).unwrap_or_default();
                Ok(json!(AddResponse {
                    sum: parsed.a + parsed.b,
                }))
            },
        )
        .with_parameters_schema::<AddArgs>()
        .with_response_schema::<AddResponse>(),
    )
}

fn utc_time_tool() -> Arc<dyn Tool> {
    Arc::new(
        FunctionTool::new(
            "utc_time",
            "Returns the current UTC timestamp in RFC3339 and unix seconds.",
            |_ctx: Arc<dyn ToolContext>, args| async move {
                let _ = serde_json::from_value::<UtcTimeArgs>(args).unwrap_or_default();
                let now = Utc::now();
                Ok(json!(UtcTimeResponse {
                    utc_rfc3339: now.to_rfc3339(),
                    unix_seconds: now.timestamp(),
                }))
            },
        )
        .with_parameters_schema::<UtcTimeArgs>()
        .with_response_schema::<UtcTimeResponse>(),
    )
}

/// Baseline ADK function tools for Sprint 3.
pub fn build_baseline_tools() -> Vec<Arc<dyn Tool>> {
    vec![echo_tool(), add_tool(), utc_time_tool()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn baseline_tools_are_non_empty_with_explicit_schemas() {
        let tools = build_baseline_tools();
        assert_eq!(tools.len(), 3);

        let names: Vec<&str> = tools.iter().map(|t| t.name()).collect();
        assert_eq!(names, vec!["echo_text", "add_numbers", "utc_time"]);

        for tool in tools {
            assert!(tool.parameters_schema().is_some());
            assert!(tool.response_schema().is_some());
        }
    }
}
