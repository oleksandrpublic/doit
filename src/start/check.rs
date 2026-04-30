use crate::config_struct::{AI_DIR, KNOWLEDGE_DIR, LOGS_DIR, PROMPTS_DIR, STATE_DIR};
use std::path::Path;

const REQUIRED_AI_SUBDIRS: &[&str] = &[
    STATE_DIR,
    LOGS_DIR,
    KNOWLEDGE_DIR,
    PROMPTS_DIR,
    "tools",
    "screenshots",
];

/// Timeout for each consistency-check Rhai script.
/// 5 seconds is generous for file-reading + regex work on any real repo.
const CHECK_SCRIPT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CheckStatus {
    Ok,
    Warn,
    Fail,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CheckItem {
    pub label: String,
    pub status: CheckStatus,
    pub detail: String,
}

impl CheckItem {
    pub(crate) fn ok(label: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            status: CheckStatus::Ok,
            detail: detail.into(),
        }
    }

    pub(crate) fn warn(label: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            status: CheckStatus::Warn,
            detail: detail.into(),
        }
    }

    pub(crate) fn fail(label: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            status: CheckStatus::Fail,
            detail: detail.into(),
        }
    }

    pub(crate) fn is_ok(&self) -> bool {
        !matches!(self.status, CheckStatus::Fail)
    }
}

pub async fn cmd_check(repo: &str, config_arg: &str) -> anyhow::Result<()> {
    let repo_path = Path::new(repo);
    let loaded = super::run_support::load_run_config(config_arg, repo_path);
    super::setup_logging(&loaded.config, None)?;

    let mut results = vec![CheckItem::ok("config load", loaded.source.clone())];

    let config_valid = match loaded.config.validate() {
        Ok(()) => {
            results.push(CheckItem::ok(
            "config validate",
            format!("model={}, backend={:?}", loaded.config.model, loaded.config.llm_backend),
            ));
            true
        }
        Err(err) => {
            results.push(CheckItem::fail("config validate", err.to_string()));
            false
        }
    };

    if config_valid {
        match loaded.config.validate_runtime().await {
            Ok(()) => {
                match loaded.config.llm_backend {
                    crate::config_struct::BackendKind::Ollama => {
                        results.push(CheckItem::ok(
                            "runtime validate",
                            format!("reachable via {}", loaded.config.llm_url),
                        ));
                    }
                    crate::config_struct::BackendKind::OpenAI
                    | crate::config_struct::BackendKind::Anthropic => {
                        results.push(CheckItem::warn(
                            "runtime validate",
                            format!(
                                "backend {:?}: API key present, but model reachability not probed. \
                                 A passing check does not guarantee the model '{}' exists.",
                                loaded.config.llm_backend,
                                loaded.config.model,
                            ),
                        ));
                    }
                }
            }
            Err(err) => results.push(CheckItem::fail("runtime validate", err.to_string())),
        }
    } else {
        results.push(CheckItem::fail(
            "runtime validate",
            "skipped because config validation failed",
        ));
    }

    // Check optional tool groups prerequisites
    for group in &loaded.config.tool_groups {
        match group.as_str() {
            "browser" => {
                let browser_cfg = &loaded.config.browser;
                if let Some(url) = browser_cfg.awp_url.as_deref().filter(|s| !s.is_empty()) {
                    // AWP uses WebSocket transport: valid schemes are ws:// and wss://.
                    // http:// and https:// are also accepted because AwpClient::new()
                    // converts them to ws:// / wss:// automatically.
                    let valid_scheme = url.starts_with("ws://")
                        || url.starts_with("wss://")
                        || url.starts_with("http://")
                        || url.starts_with("https://");
                    if valid_scheme {
                        results.push(CheckItem::ok(
                            "tool_group: browser (awp)",
                            format!("awp_url = {url} (reachability not probed at check time)"),
                        ));
                    } else {
                        results.push(CheckItem::fail(
                            "tool_group: browser (awp)",
                            format!(
                                "awp_url '{url}' must start with ws://, wss://, http://, or https://"
                            ),
                        ));
                    }
                } else {
                    results.push(CheckItem::warn(
                        "tool_group: browser",
                        "awp_url is not set in [browser]; browser tools will fail at runtime. \
                         Start the server with: plasmate serve --protocol awp --host 127.0.0.1 --port 9222",
                    ));
                }
            }
            "github" => {
                let has_token = std::env::var("GITHUB_TOKEN").is_ok();
                if has_token {
                    results.push(CheckItem::ok(
                        "tool_group: github",
                        "GITHUB_TOKEN env var is set",
                    ));
                } else {
                    results.push(CheckItem::warn(
                        "tool_group: github",
                        "GITHUB_TOKEN env var is not set; \
                         unauthenticated GitHub API requests are rate-limited (60/hour)",
                    ));
                }
            }
            "background" => {
                let state_dir = repo_path.join(".ai").join("state");
                if state_dir.exists() {
                    results.push(CheckItem::ok(
                        "tool_group: background",
                        ".ai/state/ directory exists",
                    ));
                } else {
                    results.push(CheckItem::warn(
                        "tool_group: background",
                        ".ai/state/ does not exist yet; run `do_it init` first",
                    ));
                }
            }
            _ => {}
        }
    }

    let ai_issues = collect_ai_structure_issues(repo_path);
    if ai_issues.is_empty() {
        results.push(CheckItem::ok(
            ".ai structure",
            "workspace layout looks complete",
        ));
    } else {
        results.push(CheckItem::fail(".ai structure", ai_issues.join("; ")));
    }

    // ── Rhai consistency scripts ───────────────────────────────────────────
    results.extend(run_consistency_scripts(repo_path).await);

    println!("{}", format_check_report(repo_path, &results));

    if results.iter().all(|item| item.is_ok()) {
        Ok(())
    } else {
        anyhow::bail!("do_it check failed")
    }
}

/// Run `check_dead_tools.rhai` and `check_prompt_sync.rhai` and return
/// CheckItems describing the outcome of each script.
async fn run_consistency_scripts(repo_root: &Path) -> Vec<CheckItem> {
    let scripts_dir = repo_root.join("scripts");
    let dead_tools_path = scripts_dir.join("check_dead_tools.rhai");
    let prompt_sync_path = scripts_dir.join("check_prompt_sync.rhai");

    if !scripts_dir.exists() {
        return vec![];
    }

    let mut items = Vec::new();

    items.push(run_one_script(
        "check_dead_tools",
        &dead_tools_path,
        repo_root,
        interpret_dead_tools_result,
    ).await);

    items.push(run_one_script(
        "check_prompt_sync",
        &prompt_sync_path,
        repo_root,
        interpret_prompt_sync_result,
    ).await);

    items
}

async fn run_one_script(
    label: &'static str,
    script_path: &Path,
    workdir: &Path,
    interpret: fn(&str) -> CheckItem,
) -> CheckItem {
    let script_text = match std::fs::read_to_string(script_path) {
        Ok(t) => t,
        Err(e) => {
            return CheckItem::warn(
                label,
                format!("script not found or unreadable ({}): {e}", script_path.display()),
            );
        }
    };

    let workdir = workdir.to_path_buf();
    let handle = tokio::task::spawn_blocking(move || {
        crate::tools::scripting::execute_script_readonly(&script_text, &workdir)
    });

    let tool_result = match tokio::time::timeout(CHECK_SCRIPT_TIMEOUT, handle).await {
        Ok(Ok(result)) => result,
        Ok(Err(e)) => {
            return CheckItem::fail(label, format!("script task panicked: {e}"));
        }
        Err(_) => {
            return CheckItem::fail(
                label,
                format!("script timed out after {}s", CHECK_SCRIPT_TIMEOUT.as_secs()),
            );
        }
    };

    if !tool_result.success {
        return CheckItem::fail(label, tool_result.output);
    }

    interpret(&tool_result.output)
}

fn interpret_dead_tools_result(output: &str) -> CheckItem {
    let missing = extract_array_from_output(output, "missing_in_core");
    let extra = extract_array_from_output(output, "extra_in_core");

    if missing.is_empty() && extra.is_empty() {
        CheckItem::ok(
            "check_dead_tools",
            "spec.rs and core.rs dispatch arms are in sync",
        )
    } else {
        let mut parts = Vec::new();
        if !missing.is_empty() {
            parts.push(format!("in spec but not dispatched: {}", missing.join(", ")));
        }
        if !extra.is_empty() {
            parts.push(format!("dispatched but not in spec: {}", extra.join(", ")));
        }
        CheckItem::fail("check_dead_tools", parts.join("; "))
    }
}

fn interpret_prompt_sync_result(output: &str) -> CheckItem {
    let mismatched = extract_array_from_output(output, "mismatched_roles");

    if mismatched.is_empty() {
        CheckItem::ok(
            "check_prompt_sync",
            "all role prompts match their tool allowlists in spec.rs",
        )
    } else {
        CheckItem::fail(
            "check_prompt_sync",
            format!("prompt/spec mismatch in roles: {}", mismatched.join(", ")),
        )
    }
}

fn extract_array_from_output(output: &str, key: &str) -> Vec<String> {
    let search = format!("{key}: [");
    let start = match output.find(&search) {
        Some(pos) => pos + search.len(),
        None => return vec![],
    };
    let rest = &output[start..];
    let end = match rest.find(']') {
        Some(pos) => pos,
        None => return vec![],
    };
    let inner = rest[..end].trim();
    if inner.is_empty() {
        return vec![];
    }
    inner
        .split(", ")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

pub(crate) fn collect_ai_structure_issues(repo_root: &Path) -> Vec<String> {
    let ai_dir = repo_root.join(AI_DIR);
    let mut issues = Vec::new();

    if !ai_dir.exists() {
        issues.push("missing .ai/".to_string());
        return issues;
    }
    if !ai_dir.is_dir() {
        issues.push(".ai exists but is not a directory".to_string());
        return issues;
    }

    for subdir in REQUIRED_AI_SUBDIRS {
        let path = ai_dir.join(subdir);
        if !path.exists() {
            issues.push(format!("missing .ai/{subdir}/"));
        } else if !path.is_dir() {
            issues.push(format!(".ai/{subdir} exists but is not a directory"));
        }
    }

    let project_toml = ai_dir.join("project.toml");
    if !project_toml.exists() {
        issues.push("missing .ai/project.toml".to_string());
    } else if !project_toml.is_file() {
        issues.push(".ai/project.toml exists but is not a file".to_string());
    }

    issues
}

pub(crate) fn format_check_report(repo_root: &Path, results: &[CheckItem]) -> String {
    let display_root = repo_root
        .canonicalize()
        .unwrap_or_else(|_| repo_root.to_path_buf());
    let mut lines = vec![
        "╭─ do_it check ───────────────────────────────────────────────╮".to_string(),
        format!("│ repo: {}", display_root.display()),
        "╰────────────────────────────────────────────────────────────╯".to_string(),
        String::new(),
    ];

    for item in results {
        let marker = match item.status {
            CheckStatus::Ok   => "ok  ",
            CheckStatus::Warn => "warn",
            CheckStatus::Fail => "fail",
        };
        lines.push(format!("  [{marker}] {}", item.label));
        lines.push(format!("         {}", item.detail));
    }

    let has_fail = results.iter().any(|i| matches!(i.status, CheckStatus::Fail));
    let has_warn = results.iter().any(|i| matches!(i.status, CheckStatus::Warn));
    let summary = if has_fail {
        "FAIL"
    } else if has_warn {
        "PASS (with warnings)"
    } else {
        "PASS"
    };
    lines.push(String::new());
    lines.push(format!("  Summary: {summary}"));
    if has_warn && !has_fail {
        lines.push("  Note: [warn] items do not block operation but should be reviewed.".to_string());
    }
    lines.join("\n")
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_array_finds_non_empty_list() {
        let output = "#{missing_in_core: [foo, bar], extra_in_core: []}";
        assert_eq!(
            extract_array_from_output(output, "missing_in_core"),
            vec!["foo", "bar"]
        );
        assert!(extract_array_from_output(output, "extra_in_core").is_empty());
    }

    #[test]
    fn extract_array_returns_empty_when_key_absent() {
        let output = "#{other_key: [x]}";
        assert!(extract_array_from_output(output, "missing_in_core").is_empty());
    }

    #[test]
    fn interpret_dead_tools_ok_when_both_lists_empty() {
        let output = "#{missing_in_core: [], extra_in_core: [], runtime_tool_count: 5}";
        let item = interpret_dead_tools_result(output);
        assert_eq!(item.status, CheckStatus::Ok);
    }

    #[test]
    fn interpret_dead_tools_fail_when_missing() {
        let output = "#{missing_in_core: [some_tool], extra_in_core: []}";
        let item = interpret_dead_tools_result(output);
        assert_eq!(item.status, CheckStatus::Fail);
        assert!(item.detail.contains("some_tool"));
    }

    #[test]
    fn interpret_prompt_sync_ok_when_no_mismatches() {
        let output = "#{mismatched_roles: [], role_reports: #{}}";
        let item = interpret_prompt_sync_result(output);
        assert_eq!(item.status, CheckStatus::Ok);
    }

    #[test]
    fn interpret_prompt_sync_fail_when_mismatches_present() {
        let output = "#{mismatched_roles: [boss, qa], role_reports: #{}}";
        let item = interpret_prompt_sync_result(output);
        assert_eq!(item.status, CheckStatus::Fail);
        assert!(item.detail.contains("boss"));
        assert!(item.detail.contains("qa"));
    }

    #[tokio::test]
    async fn scripts_skipped_when_scripts_dir_absent() {
        let dir = tempfile::TempDir::new().unwrap();
        let items = run_consistency_scripts(dir.path()).await;
        assert!(items.is_empty());
    }

    #[tokio::test]
    async fn script_warns_when_file_missing() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::create_dir(dir.path().join("scripts")).unwrap();
        let items = run_consistency_scripts(dir.path()).await;
        assert_eq!(items.len(), 2);
        assert!(items.iter().all(|i| i.status == CheckStatus::Warn));
    }

    // ── AWP URL scheme validation ──────────────────────────────────────────

    #[test]
    fn awp_ws_scheme_is_valid() {
        // Regression: ws:// was incorrectly rejected before this fix.
        for url in &["ws://127.0.0.1:9222", "wss://example.com:9222",
                     "http://127.0.0.1:9222", "https://example.com:9222"] {
            let valid = url.starts_with("ws://")
                || url.starts_with("wss://")
                || url.starts_with("http://")
                || url.starts_with("https://");
            assert!(valid, "URL '{url}' should be accepted");
        }
    }

    #[test]
    fn awp_invalid_scheme_is_rejected() {
        for url in &["ftp://host:9222", "tcp://host:9222", "127.0.0.1:9222", ""] {
            let valid = url.starts_with("ws://")
                || url.starts_with("wss://")
                || url.starts_with("http://")
                || url.starts_with("https://");
            assert!(!valid, "URL '{url}' should be rejected");
        }
    }
}
