use crate::tools::utils::resolve_safe_path;
use crate::tools::{ToolDispatchKind, canonical_tool_name};
use anyhow::{Result, bail};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::Path;

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

impl TelegramConfig {
    pub async fn send_message(&self, message: &str) -> Result<String> {
        let (token, chat_id) = match (&self.token, &self.chat_id) {
            (Some(t), Some(c)) => (t, c),
            _ => return Ok("Telegram not configured".to_string()),
        };

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .map_err(|e| anyhow::anyhow!("Failed to create HTTP client: {e}"))?;

        let url = format!("https://api.telegram.org/bot{}/sendMessage", token);
        let payload = serde_json::json!({
            "chat_id": chat_id,
            "text": message,
            "parse_mode": "Markdown"
        });

        match client.post(&url).json(&payload).send().await {
            Ok(resp) if resp.status().is_success() => {
                Ok(format!("Telegram notification sent to chat {chat_id}"))
            }
            Ok(resp) => {
                let status = resp.status();
                let body = resp.text().await.unwrap_or_default();
                Ok(format!("Telegram API returned {status} {body}"))
            }
            Err(e) => Ok(format!("Telegram request failed: {e}")),
        }
    }
}

pub fn str_arg(args: &Value, key: &str) -> Result<String> {
    args.get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| anyhow::anyhow!("Missing arg: {key}"))
}

pub fn take_arg(args: &Value, key: &str) -> Result<Value> {
    args.get(key)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("Missing arg: {key}"))
}

pub fn resolve(root: &Path, rel: &str) -> Result<std::path::PathBuf> {
    resolve_safe_path(root, rel)
}

pub fn chrono_now() -> String {
    Utc::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

/// Convenience shim for call sites that don't have a config (tests, simple dispatch).
pub async fn dispatch(
    tool: &str,
    args: &Value,
    root: &Path,
    allowlist: &[String],
) -> Result<ToolResult> {
    dispatch_with_depth(
        tool,
        args,
        root,
        0,
        allowlist,
        &crate::config_struct::AgentConfig::default(),
    )
    .await
}

pub async fn dispatch_with_depth(
    tool: &str,
    args: &Value,
    root: &Path,
    depth: usize,
    allowlist: &[String],
    cfg: &crate::config_struct::AgentConfig,
) -> Result<ToolResult> {
    let normalized_tool = tool.trim();
    let canonical_tool = match canonical_tool_name(normalized_tool) {
        Some(name) => name,
        None => bail!("Unknown tool: {normalized_tool}"),
    };

    if matches!(
        crate::tools::find_tool_spec(canonical_tool).map(|spec| spec.dispatch),
        Some(ToolDispatchKind::AgentLoop)
    ) {
        bail!("Tool '{canonical_tool}' is handled by the agent loop, not runtime dispatch");
    }

    match canonical_tool {
        "read_file" => crate::tools::file_ops::read_file(args, root),
        "open_file_region" => crate::tools::file_ops::open_file_region(args, root),
        "read_test_failure" => crate::tools::file_ops::read_test_failure(args, root),
        "write_file" => crate::tools::file_ops::write_file(args, root),
        "str_replace" => crate::tools::file_ops::str_replace(args, root),
        "apply_patch_preview" => crate::tools::file_ops::apply_patch_preview(args, root),
        "str_replace_multi" => crate::tools::file_ops::str_replace_multi(args, root),
        "str_replace_fuzzy" => crate::tools::file_ops::str_replace_fuzzy(args, root),
        "list_dir" => crate::tools::file_ops::list_dir(args, root),
        "find_files" => crate::tools::file_ops::find_files(args, root),
        "search_in_files" => crate::tools::file_ops::search_in_files(args, root),
        "run_command" => crate::tools::commands::run_command(args, root, allowlist).await,
        "format_changed_files_only" => {
            crate::tools::commands::format_changed_files_only(args, root).await
        }
        "run_targeted_test" => crate::tools::commands::run_targeted_test(args, root).await,
        "web_search" => crate::tools::web::web_search(args).await,
        "fetch_url" => crate::tools::web::fetch_url(args).await,
        "ask_human" => crate::tools::human::ask_human(args).await,
        "notify" => crate::tools::human::notify(args).await,
        "get_symbols" => crate::tools::code_analysis::get_symbols(args, root),
        "outline" => crate::tools::code_analysis::outline(args, root),
        "get_signature" => crate::tools::code_analysis::get_signature(args, root),
        "find_references" => crate::tools::code_analysis::find_references(args, root),
        "git_status" => crate::tools::git::git_status(args, root).await,
        "git_commit" => crate::tools::git::git_commit(args, root).await,
        "git_log" => crate::tools::git::git_log(args, root).await,
        "git_stash" => crate::tools::git::git_stash(args, root).await,
        "git_pull" => crate::tools::git::git_pull(args, root).await,
        "git_push" => crate::tools::git::git_push(args, root).await,
        "github_api" => crate::tools::web::github_api(args).await,
        "test_coverage" => crate::tools::test_coverage::test_coverage(args, root).await,
        "run_script" => crate::tools::scripting::run_script(args, root).await,
        "browser_action" => crate::tools::browser::browser_action(args, root).await,
        "browser_get_text" => crate::tools::browser::browser_get_text(args, root).await,
        "browser_navigate" => crate::tools::browser::browser_navigate(args, root).await,
        "screenshot" => crate::tools::browser::browser_screenshot(args, root).await,
        "tool_request" => crate::tools::self_improvement::tool_request(args, root).await,
        "capability_gap" => crate::tools::self_improvement::capability_gap(args, root).await,
        "memory_read" => crate::tools::memory::memory_read(args, root).await,
        "memory_write" => crate::tools::memory::memory_write(args, root).await,
        "memory_delete" => crate::tools::memory::memory_delete(args, root).await,
        "tree" => crate::tools::workspace::tree(args, root).await,
        "project_map" => crate::tools::workspace::project_map(args, root).await,
        "find_entrypoints" => crate::tools::workspace::find_entrypoints(args, root).await,
        "trace_call_path" => crate::tools::workspace::trace_call_path(args, root).await,
        "diff_repo" => crate::tools::workspace::diff_repo(args, root).await,
        // Box::pin breaks the async recursion cycle:
        // dispatch_with_depth → spawn_agent → SweAgent::run → dispatch_with_depth
        "spawn_agent" => Box::pin(crate::agent::spawn::spawn_agent(args, root, depth, cfg)).await,
        "spawn_agents" => Box::pin(crate::agent::spawn::spawn_agents(args, root, depth, cfg)).await,
        "run_background" => crate::tools::background::run_background(args, root).await,
        "process_status" => crate::tools::background::process_status(args, root).await,
        "process_list" => crate::tools::background::process_list(args, root).await,
        "process_kill" => crate::tools::background::process_kill(args, root).await,
        _ => bail!("Unknown tool: {canonical_tool}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::path::PathBuf;

    fn test_root() -> PathBuf {
        std::env::temp_dir().join("do_it_test")
    }

    #[test]
    fn test_str_arg_missing() {
        let args = json!({ "other": "value" });
        let result = str_arg(&args, "missing");
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Missing arg: missing")
        );
    }

    #[test]
    fn test_str_arg_wrong_type() {
        let args = json!({ "key": 123 });
        let result = str_arg(&args, "key");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Missing arg: key"));
    }

    #[test]
    fn test_str_arg_success() {
        let args = json!({ "key": "value" });
        let result = str_arg(&args, "key");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "value");
    }

    #[test]
    fn test_take_arg_missing() {
        let args = json!({ "other": "value" });
        let result = take_arg(&args, "missing");
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Missing arg: missing")
        );
    }

    #[test]
    fn test_take_arg_success() {
        let args = json!({ "key": { "nested": "value" } });
        let result = take_arg(&args, "key");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), json!({ "nested": "value" }));
    }

    #[test]
    fn test_resolve_success() {
        let root = test_root();
        let result = resolve(&root, "subdir/file.txt");
        assert!(result.is_ok());
        let path = result.unwrap();
        assert!(path.to_string_lossy().contains("subdir"));
        assert!(path.to_string_lossy().contains("file.txt"));
    }

    #[test]
    fn test_resolve_path_traversal_blocked() {
        let root = test_root();
        std::fs::create_dir_all(&root).ok();
        std::fs::create_dir_all(root.join("subdir")).ok();
        std::fs::write(root.join("subdir").join("file.txt"), "test").ok();
        let result = resolve(&root, "../outside_file.txt");
        assert!(result.is_err(), "Path traversal should be blocked");
    }

    #[test]
    fn test_chrono_now_format() {
        let now = chrono_now();
        assert!(!now.is_empty());
        assert!(now.contains('-'));
        assert!(now.contains(':'));
    }
}

pub use crate::tools::tool_result::ToolResult;
