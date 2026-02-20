use std::path::{Component, Path, PathBuf};
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

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
struct ReadWorkspaceFileArgs {
    #[serde(default)]
    path: String,
    #[serde(default)]
    max_bytes: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
struct ReadWorkspaceFileResponse {
    path: String,
    bytes_read: usize,
    truncated: bool,
    content: String,
}

#[derive(Debug, Clone)]
pub struct WorkspaceToolPolicy {
    pub workspace_root: PathBuf,
    pub forbidden_paths: Vec<String>,
    pub max_read_bytes: usize,
}

impl Default for WorkspaceToolPolicy {
    fn default() -> Self {
        Self {
            workspace_root: PathBuf::from("."),
            forbidden_paths: vec![".git".to_string(), "target".to_string()],
            max_read_bytes: 16_384,
        }
    }
}

fn resolve_workspace_file_path(
    policy: &WorkspaceToolPolicy,
    requested_path: &str,
) -> Result<PathBuf, String> {
    let trimmed = requested_path.trim();
    if trimmed.is_empty() {
        return Err("Path is empty".to_string());
    }

    let requested = PathBuf::from(trimmed);
    if requested.is_absolute() {
        return Err("Absolute paths are forbidden".to_string());
    }

    if requested
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err("Parent path traversal '..' is forbidden".to_string());
    }

    let workspace_root = policy
        .workspace_root
        .canonicalize()
        .map_err(|e| format!("Workspace root is not accessible: {}", e))?;
    let candidate = workspace_root.join(&requested);
    let resolved = candidate
        .canonicalize()
        .map_err(|e| format!("Failed to access '{}': {}", candidate.display(), e))?;

    if !resolved.starts_with(&workspace_root) {
        return Err("Resolved path escapes workspace root".to_string());
    }

    let relative = resolved
        .strip_prefix(&workspace_root)
        .unwrap_or(Path::new(""))
        .to_path_buf();
    for forbidden in &policy.forbidden_paths {
        let trimmed = forbidden.trim();
        if trimmed.is_empty() {
            continue;
        }
        let forbidden_path = Path::new(trimmed);
        let blocked = if forbidden_path.is_absolute() {
            resolved.starts_with(forbidden_path)
        } else {
            relative.starts_with(forbidden_path)
        };
        if blocked {
            return Err(format!("Path is forbidden by policy '{}'", trimmed));
        }
    }

    Ok(resolved)
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

fn read_workspace_file_tool(policy: WorkspaceToolPolicy) -> Arc<dyn Tool> {
    Arc::new(
        FunctionTool::new(
            "read_workspace_file",
            "Reads a UTF-8 preview of a file scoped to the configured workspace root.",
            move |_ctx: Arc<dyn ToolContext>, args| {
                let policy = policy.clone();
                async move {
                    let parsed =
                        serde_json::from_value::<ReadWorkspaceFileArgs>(args).unwrap_or_default();
                    let resolved_path = resolve_workspace_file_path(&policy, &parsed.path)
                        .map_err(adk_core::AdkError::Tool)?;

                    let bytes = tokio::fs::read(&resolved_path).await.map_err(|e| {
                        adk_core::AdkError::Tool(format!(
                            "Failed to read '{}': {}",
                            resolved_path.display(),
                            e
                        ))
                    })?;

                    let max_bytes = parsed
                        .max_bytes
                        .unwrap_or(policy.max_read_bytes)
                        .max(1)
                        .min(policy.max_read_bytes);
                    let take_len = bytes.len().min(max_bytes);
                    let truncated = bytes.len() > take_len;
                    let content = String::from_utf8_lossy(&bytes[..take_len]).to_string();

                    Ok(json!(ReadWorkspaceFileResponse {
                        path: parsed.path,
                        bytes_read: take_len,
                        truncated,
                        content,
                    }))
                }
            },
        )
        .with_parameters_schema::<ReadWorkspaceFileArgs>()
        .with_response_schema::<ReadWorkspaceFileResponse>(),
    )
}

/// Baseline ADK function tools for Sprint 3.
pub fn build_baseline_tools() -> Vec<Arc<dyn Tool>> {
    build_baseline_tools_with_workspace(WorkspaceToolPolicy::default())
}

/// Baseline ADK tools with workspace-scoped file-read capability.
pub fn build_baseline_tools_with_workspace(policy: WorkspaceToolPolicy) -> Vec<Arc<dyn Tool>> {
    vec![
        echo_tool(),
        add_tool(),
        utc_time_tool(),
        read_workspace_file_tool(policy),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn baseline_tools_are_non_empty_with_explicit_schemas() {
        let tools = build_baseline_tools();
        assert_eq!(tools.len(), 4);

        let names: Vec<&str> = tools.iter().map(|t| t.name()).collect();
        assert_eq!(
            names,
            vec![
                "echo_text",
                "add_numbers",
                "utc_time",
                "read_workspace_file"
            ]
        );

        for tool in tools {
            assert!(tool.parameters_schema().is_some());
            assert!(tool.response_schema().is_some());
        }
    }

    #[test]
    fn resolve_workspace_file_path_rejects_parent_traversal() {
        let policy = WorkspaceToolPolicy::default();
        let err = resolve_workspace_file_path(&policy, "../secret.txt").unwrap_err();
        assert!(err.contains("Parent path traversal"));
    }

    #[test]
    fn resolve_workspace_file_path_rejects_forbidden_relative_prefix() {
        let workspace = std::env::temp_dir().join(format!(
            "liveclaw-tools-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(workspace.join(".git")).unwrap();
        let file_path = workspace.join(".git").join("config");
        std::fs::write(&file_path, "secret").unwrap();

        let policy = WorkspaceToolPolicy {
            workspace_root: workspace.clone(),
            forbidden_paths: vec![".git".to_string()],
            max_read_bytes: 1024,
        };

        let err = resolve_workspace_file_path(&policy, ".git/config").unwrap_err();
        assert!(err.contains("forbidden"));

        let _ = std::fs::remove_dir_all(workspace);
    }

    #[test]
    fn resolve_workspace_file_path_accepts_workspace_file() {
        let workspace = std::env::temp_dir().join(format!(
            "liveclaw-tools-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(workspace.join("docs")).unwrap();
        let file_path = workspace.join("docs").join("notes.txt");
        std::fs::write(&file_path, "hello").unwrap();

        let policy = WorkspaceToolPolicy {
            workspace_root: workspace.clone(),
            forbidden_paths: vec![".git".to_string()],
            max_read_bytes: 1024,
        };

        let resolved = resolve_workspace_file_path(&policy, "docs/notes.txt").unwrap();
        assert_eq!(resolved, file_path.canonicalize().unwrap());

        let _ = std::fs::remove_dir_all(workspace);
    }
}
