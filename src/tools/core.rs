use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};
use chrono::Utc;

#[derive(Debug, Clone)]
pub struct ToolResult {
    pub output: String,
    pub success: bool,
}

impl ToolResult {
    pub fn ok(output: impl Into<String>) -> Self {
        Self { output: output.into(), success: true }
    }
}

#[derive(Debug, Deserialize, Serialize)]
pub struct LlmAction {
    pub thought: String,
    pub tool: String,
    pub args: Value,
}

#[derive(Debug, Clone, Default)]
pub struct TelegramConfig {
    pub token: Option<String>,
    pub chat_id: Option<String>,
}

pub fn str_arg(args: &Value, key: &str) -> Result<String> {
    args.get(key).and_then(|v| v.as_str()).map(|s| s.to_string())
        .ok_or_else(|| anyhow::anyhow!("Missing arg: {key}"))
}

pub fn take_arg(args: &Value, key: &str) -> Result<Value> {
    args.get(key).cloned()
        .ok_or_else(|| anyhow::anyhow!("Missing arg: {key}"))
}

pub fn resolve(root: &Path, rel: &str) -> Result<std::path::PathBuf> {
    let path = Path::new(rel);
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    };
    
    // Normalize the path (resolve .. and . components)
    let mut normalized = PathBuf::new();
    for component in joined.components() {
        match component {
            std::path::Component::ParentDir => {
                if !normalized.pop() {
                    bail!("Path traversal detected: {} escapes repository root", rel);
                }
            }
            std::path::Component::CurDir => {}
            _ => normalized.push(component),
        }
    }
    
    // Security check: ensure resolved path is within root directory
    let canonical_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let canonical_resolved = normalized.canonicalize().unwrap_or_else(|_| normalized.clone());
    
    if !canonical_resolved.starts_with(&canonical_root) {
        bail!("Path traversal detected: {} escapes repository root", rel);
    }
    
    Ok(normalized)
}

pub fn chrono_now() -> String {
    Utc::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

pub async fn dispatch(tool: &str, args: &Value, root: &Path) -> Result<ToolResult> {
    match tool {
        "read_file" => crate::tools::file_ops::read_file(args, root),
        "write_file" => crate::tools::file_ops::write_file(args, root),
        "str_replace" => crate::tools::file_ops::str_replace(args, root),
        "list_dir" => crate::tools::file_ops::list_dir(args, root),
        "find_files" => crate::tools::file_ops::find_files(args, root),
        "search_in_files" => crate::tools::file_ops::search_in_files(args, root),
        "run_command" => crate::tools::commands::run_command(args, root).await,
        "web_search" => crate::tools::web::web_search(args).await,
        "web_fetch" => crate::tools::web::fetch_url(args).await,
        "ask_human" => crate::tools::human::ask_human(args).await,
        "analyze_code" => crate::tools::code_analysis::get_symbols(args, root),
        "git_status" => crate::tools::git::git_status(args, root).await,
        "git_commit" => crate::tools::git::git_commit(args, root).await,
        "test_coverage" => crate::tools::test_coverage::test_coverage(args, root).await,
        "browser_automation" => crate::tools::browser::browser_action(args, root).await,
        "self_improve" => crate::tools::self_improvement::tool_request(args, root).await,
        "memory_read" => crate::tools::memory::memory_read(args, root).await,
        "memory_write" => crate::tools::memory::memory_write(args, root).await,
        "workspace_tree" => crate::tools::workspace::tree(args, root).await,
        "workspace_diff" => crate::tools::workspace::diff_repo(args, root).await,
        "spawn_agents" => crate::tools::agents::spawn_agents(args, root).await,
        "process_list" => crate::tools::background::process_list(args, root).await,
        "process_kill" => crate::tools::background::process_kill(args, root).await,
        _ => bail!("Unknown tool: {tool}"),
    }
}
