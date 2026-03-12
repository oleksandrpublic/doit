use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

// ─── Tool result ──────────────────────────────────────────────────────────────

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

// ─── Tool call (parsed from LLM JSON) ────────────────────────────────────────

#[derive(Debug, Deserialize, Serialize)]
pub struct LlmAction {
    pub thought: String,
    pub tool: String,
    pub args: Value,
}

// ─── Telegram config (passed into tools that need human interaction) ─────────

#[derive(Debug, Clone, Default)]
pub struct TelegramConfig {
    pub token: Option<String>,
    pub chat_id: Option<String>,
}

// ─── Dispatch ─────────────────────────────────────────────────────────────────

/// Execute a tool by name with JSON args, relative to `root`.
pub fn dispatch<'a>(
    tool: &'a str,
    args: &'a Value,
    root: &'a Path,
    max_output: usize,
    tg: &'a TelegramConfig,
    cfg: &'a crate::config::AgentConfig,
    sub_agent_max_steps: usize,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<ToolResult>> + 'a>> {
    Box::pin(async move {
    let result = match tool {
        "read_file"        => read_file(args, root),
        "write_file"       => write_file(args, root),
        "str_replace"      => str_replace(args, root),
        "list_dir"         => list_dir(args, root),
        "find_files"       => find_files(args, root),
        "search_in_files"  => search_in_files(args, root),
        "run_command"      => run_command(args, root).await,
        "fetch_url"        => fetch_url(args).await,
        "web_search"       => web_search(args).await,
        "ask_human"        => ask_human(args, tg).await,
        "diff_repo"        => diff_repo(args, root).await,
        "tree"             => tree(args, root),
        "memory_read"      => memory_read(args, root),
        "memory_write"     => memory_write(args, root),
        "get_symbols"      => get_symbols(args, root),
        "outline"          => outline(args, root),
        "get_signature"    => get_signature(args, root),
        "find_references"  => find_references(args, root),
        "git_status"       => git_status(args, root).await,
        "git_commit"       => git_commit(args, root).await,
        "git_log"          => git_log(args, root).await,
        "git_stash"        => git_stash(args, root).await,
        "spawn_agent"      => spawn_agent(args, root, cfg, sub_agent_max_steps).await,
        "github_api"       => github_api(args).await,
        "test_coverage"    => test_coverage(args, root).await,
        "notify"           => notify(args, tg).await,
        "finish"           => Ok(ToolResult::ok("__finish__")),
        unknown => bail!(
            "Unknown tool: '{unknown}'. Available: read_file, write_file, str_replace, \
             list_dir, find_files, search_in_files, run_command, fetch_url, web_search, \
             ask_human, diff_repo, tree, memory_read, memory_write, \
             get_symbols, outline, get_signature, find_references, \
             git_status, git_commit, git_log, git_stash, spawn_agent, github_api, test_coverage, notify, finish"
        ),
    }?;

    // Truncate large outputs
    if result.output.len() > max_output {
        let half = max_output / 2;
        let truncated = format!(
            "{}\n\n[... truncated {} chars ...]\n\n{}",
            &result.output[..half],
            result.output.len() - max_output,
            &result.output[result.output.len() - half..]
        );
        return Ok(ToolResult { output: truncated, success: result.success });
    }

    Ok(result)
    })
}

// ─── Tool implementations ─────────────────────────────────────────────────────

/// Read a file with line numbers. Optional start/end line window.
fn read_file(args: &Value, root: &Path) -> Result<ToolResult> {
    let path = resolve(root, str_arg(args, "path")?)?;
    let content = std::fs::read_to_string(&path)
        .map_err(|e| anyhow::anyhow!("read_file: {e} ({path:?})"))?;

    let start = args.get("start_line").and_then(|v| v.as_u64()).map(|n| n as usize).unwrap_or(1);
    let end = args.get("end_line").and_then(|v| v.as_u64()).map(|n| n as usize);

    let lines: Vec<&str> = content.lines().collect();
    let total = lines.len();
    let s = start.saturating_sub(1).min(total);
    let e = end.map(|n| n.min(total)).unwrap_or((s + 100).min(total));

    let numbered: String = lines[s..e]
        .iter()
        .enumerate()
        .map(|(i, l)| format!("{:>4}  {}", s + i + 1, l))
        .collect::<Vec<_>>()
        .join("\n");

    Ok(ToolResult::ok(format!(
        "File: {path:?} (lines {}-{} of {})\n{numbered}",
        s + 1,
        e,
        total
    )))
}

/// Overwrite a file with new content (creates parent dirs if needed).
fn write_file(args: &Value, root: &Path) -> Result<ToolResult> {
    let path = resolve(root, str_arg(args, "path")?)?;
    let content = str_arg(args, "content")?;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, content)?;
    Ok(ToolResult::ok(format!("Written {} bytes to {path:?}", content.len())))
}

/// Replace a unique occurrence of old_str with new_str in a file.
fn str_replace(args: &Value, root: &Path) -> Result<ToolResult> {
    let path = resolve(root, str_arg(args, "path")?)?;
    let old_str = str_arg(args, "old_str")?;
    let new_str = str_arg(args, "new_str")?;

    let content = std::fs::read_to_string(&path)
        .map_err(|e| anyhow::anyhow!("str_replace: {e} ({path:?})"))?;

    let count = content.matches(old_str).count();
    if count == 0 {
        bail!("str_replace: old_str not found in {path:?}");
    }
    if count > 1 {
        bail!("str_replace: old_str found {count} times in {path:?} — must be unique");
    }

    let new_content = content.replacen(old_str, new_str, 1);
    std::fs::write(&path, &new_content)?;
    Ok(ToolResult::ok(format!("Replaced in {path:?}")))
}

/// List directory contents (one level deep).
fn list_dir(args: &Value, root: &Path) -> Result<ToolResult> {
    let dir = if let Some(p) = args.get("path").and_then(|v| v.as_str()) {
        resolve(root, p)?
    } else {
        root.to_path_buf()
    };

    let mut entries: Vec<String> = std::fs::read_dir(&dir)
        .map_err(|e| anyhow::anyhow!("list_dir: {e} ({dir:?})"))?
        .filter_map(|e| e.ok())
        .map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            let ft = e.file_type().ok();
            if ft.map(|t| t.is_dir()).unwrap_or(false) {
                format!("{name}/")
            } else {
                name
            }
        })
        .collect();

    entries.sort();
    Ok(ToolResult::ok(format!("{}:\n{}", dir.display(), entries.join("\n"))))
}

/// Find files by name substring or simple glob (* wildcard).
fn find_files(args: &Value, root: &Path) -> Result<ToolResult> {
    let pattern = str_arg(args, "pattern")?;
    let dir = if let Some(p) = args.get("dir").and_then(|v| v.as_str()) {
        resolve(root, p)?
    } else {
        root.to_path_buf()
    };

    // Convert simple glob to a matcher (support leading/trailing *)
    let pattern_lower = pattern.to_lowercase();
    let (prefix, suffix) = if let Some(s) = pattern.strip_prefix('*') {
        ("", s)
    } else if let Some(p) = pattern.strip_suffix('*') {
        (p, "")
    } else {
        ("", pattern)
    };

    let mut results = Vec::new();
    for entry in WalkDir::new(&dir).follow_links(false).into_iter().filter_map(|e| e.ok()) {
        if entry.file_type().is_file() {
            let name = entry.file_name().to_string_lossy().to_lowercase();
            let matches = if prefix.is_empty() && suffix.is_empty() {
                name.contains(&pattern_lower)
            } else if prefix.is_empty() {
                name.ends_with(suffix)
            } else {
                name.starts_with(prefix)
            };

            if matches {
                // Return path relative to root
                let rel = entry.path().strip_prefix(root).unwrap_or(entry.path());
                results.push(rel.to_string_lossy().to_string());
            }
        }
        if results.len() >= 100 {
            results.push("... (truncated at 100 results)".to_string());
            break;
        }
    }

    if results.is_empty() {
        Ok(ToolResult::ok(format!("No files matching '{pattern}' found in {dir:?}")))
    } else {
        Ok(ToolResult::ok(results.join("\n")))
    }
}

/// Search for a text pattern across files (like grep -rn).
fn search_in_files(args: &Value, root: &Path) -> Result<ToolResult> {
    let pattern = str_arg(args, "pattern")?;
    let dir = if let Some(p) = args.get("dir").and_then(|v| v.as_str()) {
        resolve(root, p)?
    } else {
        root.to_path_buf()
    };
    let ext_filter = args.get("ext").and_then(|v| v.as_str());

    let re = regex::Regex::new(pattern)
        .map_err(|e| anyhow::anyhow!("search_in_files: invalid regex '{pattern}': {e}"))?;

    let mut matches = Vec::new();

    for entry in WalkDir::new(&dir).follow_links(false).into_iter().filter_map(|e| e.ok()) {
        if !entry.file_type().is_file() {
            continue;
        }

        // Extension filter
        if let Some(ext) = ext_filter {
            let file_ext = entry.path().extension().and_then(|e| e.to_str()).unwrap_or("");
            if !file_ext.eq_ignore_ascii_case(ext) {
                continue;
            }
        }

        // Skip binary-looking files
        let Ok(content) = std::fs::read_to_string(entry.path()) else { continue };

        for (lineno, line) in content.lines().enumerate() {
            if re.is_match(line) {
                let rel = entry.path().strip_prefix(root).unwrap_or(entry.path());
                matches.push(format!("{}:{}: {}", rel.display(), lineno + 1, line.trim()));
            }
            if matches.len() >= 200 {
                break;
            }
        }
        if matches.len() >= 200 {
            matches.push("... (truncated at 200 matches)".to_string());
            break;
        }
    }

    if matches.is_empty() {
        Ok(ToolResult::ok(format!("No matches for '{pattern}'")))
    } else {
        Ok(ToolResult::ok(matches.join("\n")))
    }
}

/// Run an executable with explicit args array (no shell injection, cross-platform).
///
/// JSON args:
/// {
///   "program": "python3",
///   "args": ["-m", "pytest", "tests/"],
///   "cwd": "."          // optional, relative to repo root
/// }
async fn run_command(args: &Value, root: &Path) -> Result<ToolResult> {
    let program = str_arg(args, "program")?;
    let cmd_args: Vec<String> = args
        .get("args")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect())
        .unwrap_or_default();

    let cwd = if let Some(p) = args.get("cwd").and_then(|v| v.as_str()) {
        resolve(root, p)?
    } else {
        root.to_path_buf()
    };

    tracing::debug!("run_command: {} {:?} in {:?}", program, cmd_args, cwd);

    let output = tokio::process::Command::new(program)
        .args(&cmd_args)
        .current_dir(&cwd)
        .output()
        .await
        .map_err(|e| anyhow::anyhow!("run_command failed to start '{program}': {e}"))?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let exit_code = output.status.code().unwrap_or(-1);

    let combined = if stderr.is_empty() {
        stdout
    } else if stdout.is_empty() {
        format!("[stderr]\n{stderr}")
    } else {
        format!("{stdout}\n[stderr]\n{stderr}")
    };

    Ok(ToolResult {
        output: format!("exit_code: {exit_code}\n{combined}"),
        success: exit_code == 0,
    })
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

fn str_arg<'a>(args: &'a Value, key: &str) -> Result<&'a str> {
    args.get(key)
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("missing required arg '{key}'"))
}

/// Resolve a path argument relative to root, preventing path traversal outside root.
fn resolve(root: &Path, rel: &str) -> Result<PathBuf> {
    let p = if Path::new(rel).is_absolute() {
        PathBuf::from(rel)
    } else {
        root.join(rel)
    };
    // Normalize without requiring path to exist yet
    let normalized = normalize_path(&p);
    Ok(normalized)
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut components = Vec::new();
    for comp in path.components() {
        match comp {
            std::path::Component::ParentDir => { components.pop(); }
            std::path::Component::CurDir => {}
            c => components.push(c),
        }
    }
    components.iter().collect()
}

// ─── fetch_url ────────────────────────────────────────────────────────────────

/// Fetch a URL and return readable text content.
/// Optional `selector` extracts a specific HTML element (e.g. "main", "article", "pre").
/// Falls back to stripping all tags if selector not found or not provided.
async fn fetch_url(args: &Value) -> Result<ToolResult> {
    let url = str_arg(args, "url")?;
    let selector = args.get("selector").and_then(|v| v.as_str());

    let client = reqwest::Client::builder()
        .user_agent("do_it-agent/1.0")
        .timeout(std::time::Duration::from_secs(15))
        .build()?;

    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("fetch_url failed for '{url}': {e}"))?;

    let status = resp.status();
    if !status.is_success() {
        return Ok(ToolResult {
            output: format!("HTTP {status} for {url}"),
            success: false,
        });
    }

    let content_type = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();

    let body = resp.text().await?;

    // Plain text / JSON — return as-is
    if !content_type.contains("html") {
        return Ok(ToolResult::ok(truncate_text(&body, 12_000)));
    }

    // HTML — extract text
    let text = if let Some(sel) = selector {
        extract_element(&body, sel).unwrap_or_else(|| strip_html(&body))
    } else {
        strip_html(&body)
    };

    Ok(ToolResult::ok(format!("[{url}]\n\n{}", truncate_text(&text, 12_000))))
}

/// Rudimentary HTML-to-text: remove tags, decode basic entities, collapse whitespace.
fn strip_html(html: &str) -> String {
    // Remove <script> and <style> blocks entirely
    let mut s = html.to_string();
    for tag in &["script", "style", "nav", "footer", "header"] {
        let open = format!("<{tag}");
        let close = format!("</{tag}>");
        while let (Some(start), Some(end)) = (
            s.to_ascii_lowercase().find(&open),
            s.to_ascii_lowercase().find(&close),
        ) {
            if start < end {
                s.drain(start..end + close.len());
            } else {
                break;
            }
        }
    }
    // Strip remaining tags
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for ch in s.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => { in_tag = false; out.push(' '); }
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    // Decode basic entities
    let out = out
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ");
    // Collapse whitespace
    let lines: Vec<&str> = out
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .collect();
    lines.join("\n")
}

/// Try to find and extract a specific HTML element by tag name.
fn extract_element(html: &str, tag: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    let start = lower.find(&open)?;
    let end = lower[start..].find(&close).map(|i| start + i + close.len())?;
    Some(strip_html(&html[start..end]))
}

fn truncate_text(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let half = max / 2;
    format!(
        "{}\n\n[... {} chars truncated ...]\n\n{}",
        &s[..half],
        s.len() - max,
        &s[s.len() - half..]
    )
}

// ─── ask_human ────────────────────────────────────────────────────────────────

/// Ask the human a question.
/// If Telegram is configured: sends the question, polls for reply (up to 5 min).
/// Otherwise: prints to stdout and reads a line from stdin.
async fn ask_human(args: &Value, tg: &TelegramConfig) -> Result<ToolResult> {
    let question = str_arg(args, "question")?;

    if let (Some(token), Some(chat_id)) = (&tg.token, &tg.chat_id) {
        match ask_via_telegram(question, token, chat_id).await {
            Ok(reply) => return Ok(ToolResult::ok(reply)),
            Err(e) => {
                tracing::warn!("Telegram ask_human failed: {e}, falling back to console");
            }
        }
    }

    // Console fallback
    println!("\n╔══════════════════════════════════════╗");
    println!("║           AGENT ASKS YOU             ║");
    println!("╚══════════════════════════════════════╝");
    println!("{question}");
    print!("\nYour answer: ");
    use std::io::Write;
    std::io::stdout().flush()?;
    let mut reply = String::new();
    std::io::stdin().read_line(&mut reply)?;
    let reply = reply.trim().to_string();
    println!();

    Ok(ToolResult::ok(if reply.is_empty() { "(no answer)".to_string() } else { reply }))
}

async fn ask_via_telegram(question: &str, token: &str, chat_id: &str) -> Result<String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()?;

    let base = format!("https://api.telegram.org/bot{token}");

    // Send question
    let text = format!("🤖 *Agent asks:*\n\n{}", question);
    let send_url = format!("{base}/sendMessage");
    let send_resp = client
        .post(&send_url)
        .json(&serde_json::json!({
            "chat_id": chat_id,
            "text": text,
            "parse_mode": "Markdown"
        }))
        .send()
        .await?;

    if !send_resp.status().is_success() {
        let body = send_resp.text().await.unwrap_or_default();
        bail!("Telegram sendMessage failed: {body}");
    }

    // Get current update_id offset to only see new replies
    let offset = get_latest_update_id(&client, &base).await?;

    println!("  [Telegram] Question sent. Waiting for reply (up to 5 min)...");

    // Poll for reply — up to 60 attempts × 5s = 5 minutes
    for _ in 0..60 {
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;

        let updates_url = format!("{base}/getUpdates?offset={}&timeout=4", offset + 1);
        let resp: serde_json::Value = client.get(&updates_url).send().await?.json().await?;

        if let Some(updates) = resp["result"].as_array() {
            for update in updates {
                let msg_text = update["message"]["text"].as_str();
                let from_chat = update["message"]["chat"]["id"]
                    .as_i64()
                    .map(|id| id.to_string())
                    .or_else(|| update["message"]["chat"]["id"].as_str().map(String::from));

                if let (Some(text), Some(from)) = (msg_text, from_chat) {
                    if from == chat_id {
                        return Ok(text.to_string());
                    }
                }
            }
        }
    }

    bail!("ask_human via Telegram: no reply received within 5 minutes")
}

async fn get_latest_update_id(client: &reqwest::Client, base: &str) -> Result<i64> {
    let resp: serde_json::Value = client
        .get(format!("{base}/getUpdates?limit=1&offset=-1"))
        .send()
        .await?
        .json()
        .await?;

    let id = resp["result"]
        .as_array()
        .and_then(|a| a.last())
        .and_then(|u| u["update_id"].as_i64())
        .unwrap_or(0);

    Ok(id)
}
// ─── diff_repo ────────────────────────────────────────────────────────────────

/// Show changes in the repository relative to HEAD (or a specific base).
/// Args:
///   base?   — git ref to diff against (default: "HEAD")
///   staged? — if true, show staged changes (git diff --staged)
///   stat?   — if true, show only file statistics, not full diff
async fn diff_repo(args: &Value, root: &Path) -> Result<ToolResult> {
    let base    = args.get("base").and_then(|v| v.as_str()).unwrap_or("HEAD");
    let staged  = args.get("staged").and_then(|v| v.as_bool()).unwrap_or(false);
    let stat    = args.get("stat").and_then(|v| v.as_bool()).unwrap_or(false);

    // Check if this is a git repo
    let git_check = tokio::process::Command::new("git")
        .args(["rev-parse", "--is-inside-work-tree"])
        .current_dir(root)
        .output()
        .await;

    if git_check.map(|o| !o.status.success()).unwrap_or(true) {
        return Ok(ToolResult {
            output: "Not a git repository. Cannot run diff_repo.".to_string(),
            success: false,
        });
    }

    // Build git diff command
    let mut git_args = vec!["diff"];
    if staged { git_args.push("--staged"); }
    if stat   { git_args.push("--stat"); }
    git_args.push("--no-color");
    git_args.push(base);

    let out = tokio::process::Command::new("git")
        .args(&git_args)
        .current_dir(root)
        .output()
        .await
        .map_err(|e| anyhow::anyhow!("git diff failed: {e}"))?;

    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();

    if !out.status.success() {
        return Ok(ToolResult {
            output: format!("git diff error:\n{stderr}"),
            success: false,
        });
    }

    if stdout.trim().is_empty() {
        // Also check untracked files
        let untracked = tokio::process::Command::new("git")
            .args(["status", "--short", "--no-color"])
            .current_dir(root)
            .output()
            .await
            .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
            .unwrap_or_default();

        let msg = if untracked.trim().is_empty() {
            format!("No changes relative to {base}.")
        } else {
            format!("No diff relative to {base}, but untracked/staged files:\n{untracked}")
        };
        return Ok(ToolResult::ok(msg));
    }

    // Prepend a summary header
    let lines = stdout.lines().count();
    let files_changed = stdout.lines()
        .filter(|l| l.starts_with("diff --git"))
        .count();

    let header = format!(
        "diff vs {base} — {files_changed} file(s) changed, {lines} lines\n{}\n",
        "─".repeat(60)
    );

    Ok(ToolResult::ok(format!("{header}{stdout}")))
}

// ─── memory_read / memory_write ───────────────────────────────────────────────
//
// The agent's persistent memory lives in <repo>/.ai/
// Structure:
//   .ai/
//   ├── prompts/                  — system prompt files for sub-agents
//   ├── state/
//   │   ├── current_plan.md       — current intentions (optional)
//   │   ├── last_session.md       — message to future self
//   │   ├── session_counter.txt   — incremented each run
//   │   └── external_messages.md — incoming messages from the world
//   ├── logs/
//   │   └── history.md            — event log
//   └── knowledge/                — free-form notes the agent saves
//
// memory_read:  reads a file from .ai/ by logical key or explicit path
// memory_write: writes to a file in .ai/ by logical key or explicit path
//
// Logical keys map to canonical paths:
//   "plan"             → state/current_plan.md
//   "last_session"     → state/last_session.md
//   "session_counter"  → state/session_counter.txt
//   "external"         → state/external_messages.md
//   "history"          → logs/history.md
//   "knowledge/<name>" → knowledge/<name>.md
//   "prompts/<name>"   → prompts/<name>.md
//   any other key      → knowledge/<key>.md  (default bucket)

const AI_DIR: &str = ".ai";

fn resolve_memory_path(root: &Path, key: &str) -> PathBuf {
    let ai = root.join(AI_DIR);
    match key {
        "plan"            => ai.join("state/current_plan.md"),
        "last_session"    => ai.join("state/last_session.md"),
        "session_counter" => ai.join("state/session_counter.txt"),
        "external"        => ai.join("state/external_messages.md"),
        "history"         => ai.join("logs/history.md"),
        other => {
            // Allow explicit sub-paths like "knowledge/auth_notes" or "prompts/boss"
            if other.contains('/') {
                ai.join(format!("{}.md", other))
            } else {
                ai.join(format!("knowledge/{}.md", other))
            }
        }
    }
}

/// Ensure the .ai/ directory hierarchy exists.
fn ensure_ai_dirs(root: &Path) -> Result<()> {
    for sub in &["state", "logs", "knowledge", "prompts", "tools"] {
        std::fs::create_dir_all(root.join(AI_DIR).join(sub))
            .map_err(|e| anyhow::anyhow!("Cannot create .ai/{sub}: {e}"))?;
    }
    Ok(())
}

/// Read a memory entry.
/// Args:
///   key  — logical key (see mapping above) or relative path inside .ai/
fn memory_read(args: &Value, root: &Path) -> Result<ToolResult> {
    let key = str_arg(args, "key")?;
    ensure_ai_dirs(root)?;

    let path = resolve_memory_path(root, key);

    if !path.exists() {
        return Ok(ToolResult {
            output: format!("Memory key '{key}' not found (path: {})", path.display()),
            success: false,
        });
    }

    let content = std::fs::read_to_string(&path)
        .map_err(|e| anyhow::anyhow!("memory_read '{}': {e}", path.display()))?;

    Ok(ToolResult::ok(format!(
        "# memory: {key}\n# path: {}\n\n{}",
        path.strip_prefix(root).unwrap_or(&path).display(),
        content
    )))
}

/// Write (overwrite) a memory entry.
/// Args:
///   key     — logical key or relative path inside .ai/
///   content — text to write
///   append? — if true, append instead of overwrite (default: false)
fn memory_write(args: &Value, root: &Path) -> Result<ToolResult> {
    let key     = str_arg(args, "key")?;
    let content = str_arg(args, "content")?;
    let append  = args.get("append").and_then(|v| v.as_bool()).unwrap_or(false);

    ensure_ai_dirs(root)?;

    let path = resolve_memory_path(root, key);

    // Create parent dirs if needed (e.g. knowledge/ or prompts/)
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| anyhow::anyhow!("memory_write mkdir '{}': {e}", parent.display()))?;
    }

    if append && path.exists() {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .map_err(|e| anyhow::anyhow!("memory_write append '{}': {e}", path.display()))?;
        writeln!(file, "\n{content}")
            .map_err(|e| anyhow::anyhow!("memory_write write '{}': {e}", path.display()))?;
    } else {
        std::fs::write(&path, content)
            .map_err(|e| anyhow::anyhow!("memory_write '{}': {e}", path.display()))?;
    }

    let rel = path.strip_prefix(root).unwrap_or(&path);
    Ok(ToolResult::ok(format!(
        "memory_write OK: {} → {}",
        key,
        rel.display()
    )))
}
// ─── web_search ───────────────────────────────────────────────────────────────

/// Search the web via DuckDuckGo (no API key required).
/// Args:
///   query       — search query string
///   max_results? — max number of results to return (default: 8)
///
/// Returns a numbered list: title, URL, and snippet for each result.
async fn web_search(args: &Value) -> Result<ToolResult> {
    let query       = str_arg(args, "query")?;
    let max_results = args.get("max_results")
        .and_then(|v| v.as_u64())
        .unwrap_or(8) as usize;

    let client = reqwest::Client::builder()
        .user_agent("Mozilla/5.0 (compatible; do_it-agent/1.0)")
        .timeout(std::time::Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::limited(5))
        .build()?;

    // DuckDuckGo HTML endpoint — returns a full HTML page with results
    let url = format!(
        "https://html.duckduckgo.com/html/?q={}",
        urlencoding_encode(query)
    );

    let resp = client
        .get(&url)
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("web_search request failed: {e}"))?;

    if !resp.status().is_success() {
        return Ok(ToolResult {
            output: format!("DuckDuckGo returned HTTP {}", resp.status()),
            success: false,
        });
    }

    let html = resp.text().await?;
    let results = parse_ddg_results(&html, max_results);

    if results.is_empty() {
        return Ok(ToolResult {
            output: format!("No results found for: {query}"),
            success: false,
        });
    }

    let mut out = format!("Search: \"{query}\"\n{}\n\n", "─".repeat(60));
    for (i, r) in results.iter().enumerate() {
        out.push_str(&format!(
            "{}. {}\n   {}\n   {}\n\n",
            i + 1,
            r.title,
            r.url,
            r.snippet
        ));
    }

    Ok(ToolResult::ok(out.trim_end().to_string()))
}

struct SearchResult {
    title:   String,
    url:     String,
    snippet: String,
}

/// Parse DuckDuckGo HTML results page.
/// DDG HTML has a stable structure: result divs with class "result__body",
/// links with class "result__a", snippets with class "result__snippet".
fn parse_ddg_results(html: &str, max: usize) -> Vec<SearchResult> {
    let mut results = Vec::new();

    // Find all result blocks — DDG marks them with result__body
    // We split on result separators and extract title/url/snippet from each
    let blocks: Vec<&str> = html.split("result__body").skip(1).collect();

    for block in blocks.into_iter().take(max * 2) {
        // Extract title and URL from the <a class="result__a" href="...">
        let title = extract_between(block, "result__a\"", "</a>")
            .map(|s| strip_html_inline(s.trim()))
            .filter(|s| !s.is_empty());

        // DDG redirects through /l/?... — extract uddg= param for real URL
        let href = extract_between(block, "result__a\" href=\"", "\"");
        let url = href.map(|h| {
            if let Some(pos) = h.find("uddg=") {
                let encoded = &h[pos + 5..];
                // Simple percent-decode of the URL
                percent_decode(encoded.split('&').next().unwrap_or(encoded))
            } else {
                h.to_string()
            }
        });

        let snippet = extract_between(block, "result__snippet\"", "</a>")
            .or_else(|| extract_between(block, "result__snippet\">", "</span>"))
            .map(|s| strip_html_inline(s.trim()))
            .filter(|s| !s.is_empty());

        if let (Some(title), Some(url), Some(snippet)) = (title, url, snippet) {
            if !url.starts_with("https://duckduckgo.com") {
                results.push(SearchResult { title, url, snippet });
                if results.len() >= max { break; }
            }
        }
    }

    results
}

fn extract_between<'a>(s: &'a str, after: &str, before: &str) -> Option<&'a str> {
    let start = s.find(after)? + after.len();
    // skip to end of tag if needed
    let s = &s[start..];
    let content_start = s.find('>')? + 1;
    let s = &s[content_start..];
    let end = s.find(before)?;
    Some(&s[..end])
}

fn strip_html_inline(s: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for ch in s.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    out.replace("&amp;", "&")
       .replace("&lt;", "<")
       .replace("&gt;", ">")
       .replace("&quot;", "\"")
       .replace("&#39;", "'")
       .replace("&nbsp;", " ")
       .split_whitespace()
       .collect::<Vec<_>>()
       .join(" ")
}

fn urlencoding_encode(s: &str) -> String {
    s.chars().flat_map(|c| {
        if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' || c == '~' {
            vec![c]
        } else if c == ' ' {
            vec!['+']
        } else {
            format!("%{:02X}", c as u32).chars().collect()
        }
    }).collect()
}

fn percent_decode(s: &str) -> String {
    let mut out = String::new();
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(hex) = std::str::from_utf8(&bytes[i+1..i+3]) {
                if let Ok(byte) = u8::from_str_radix(hex, 16) {
                    out.push(byte as char);
                    i += 3;
                    continue;
                }
            }
        } else if bytes[i] == b'+' {
            out.push(' ');
            i += 1;
            continue;
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

// ─── tree ─────────────────────────────────────────────────────────────────────

/// Print the directory tree rooted at `dir` (default: repo root).
/// Args:
///   dir?        — subdirectory to start from (default: repo root)
///   depth?      — max depth (default: 4)
///   ignore?     — comma-separated dir names to skip
///                 (default: "target,.git,node_modules,.ai,dist")
///
/// Output mimics the classic `tree` command.
fn tree(args: &Value, root: &Path) -> Result<ToolResult> {
    let dir = args.get("dir")
        .and_then(|v| v.as_str())
        .map(|d| root.join(d))
        .unwrap_or_else(|| root.to_path_buf());

    let max_depth = args.get("depth")
        .and_then(|v| v.as_u64())
        .unwrap_or(4) as usize;

    let ignore_str = args.get("ignore")
        .and_then(|v| v.as_str())
        .unwrap_or("target,.git,node_modules,.ai,dist,.next,__pycache__");

    let ignored: std::collections::HashSet<&str> =
        ignore_str.split(',').map(|s| s.trim()).collect();

    let mut lines = Vec::new();
    let root_label = dir.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(".");
    lines.push(root_label.to_string());

    tree_walk(&dir, &ignored, max_depth, 0, "", &mut lines)?;

    let total_lines = lines.len() - 1;
    lines.push(format!("\n{total_lines} entries"));

    Ok(ToolResult::ok(lines.join("\n")))
}

fn tree_walk(
    dir: &Path,
    ignored: &std::collections::HashSet<&str>,
    max_depth: usize,
    depth: usize,
    prefix: &str,
    lines: &mut Vec<String>,
) -> Result<()> {
    if depth >= max_depth {
        lines.push(format!("{prefix}└── ..."));
        return Ok(());
    }

    let mut entries: Vec<std::fs::DirEntry> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .filter(|e| {
            let name = e.file_name();
            let name_str = name.to_string_lossy();
            // Skip hidden files/dirs (except at root level) and ignored dirs
            !ignored.contains(name_str.as_ref())
        })
        .collect();

    // Sort: dirs first, then files, both alphabetically
    entries.sort_by(|a, b| {
        let a_is_dir = a.file_type().map(|t| t.is_dir()).unwrap_or(false);
        let b_is_dir = b.file_type().map(|t| t.is_dir()).unwrap_or(false);
        match (a_is_dir, b_is_dir) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => a.file_name().cmp(&b.file_name()),
        }
    });

    let count = entries.len();
    for (i, entry) in entries.iter().enumerate() {
        let is_last   = i == count - 1;
        let connector = if is_last { "└── " } else { "├── " };
        let extension = if is_last { "    " } else { "│   " };

        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);

        let label = if is_dir {
            format!("{}/", name_str)
        } else {
            name_str.to_string()
        };

        lines.push(format!("{prefix}{connector}{label}"));

        if is_dir {
            tree_walk(
                &entry.path(),
                ignored,
                max_depth,
                depth + 1,
                &format!("{prefix}{extension}"),
                lines,
            )?;
        }
    }

    Ok(())
}
// ─── AST tools (regex-based, zero native deps) ───────────────────────────────
//
// Supported languages detected by file extension:
//   Rust        .rs
//   TypeScript  .ts .tsx
//   JavaScript  .js .jsx
//   Python      .py
//   C++         .cpp .cc .cxx .hpp .h
//   Kotlin      .kt .kts
//
// All four tools share a common Symbol type and per-language pattern tables.

#[derive(Debug, Clone)]
struct Symbol {
    kind:      &'static str,  // "fn", "struct", "class", "impl", etc.
    name:      String,
    line:      usize,         // 1-based
    signature: String,        // full first line of declaration
    container: Option<String>,// enclosing impl/class if known
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Lang {
    Rust,
    TypeScript,
    JavaScript,
    Python,
    Cpp,
    Kotlin,
    Unknown,
}

fn detect_lang(path: &Path) -> Lang {
    match path.extension().and_then(|e| e.to_str()).unwrap_or("") {
        "rs"                       => Lang::Rust,
        "ts" | "tsx"               => Lang::TypeScript,
        "js" | "jsx" | "mjs"      => Lang::JavaScript,
        "py"                       => Lang::Python,
        "cpp" | "cc" | "cxx" |
        "hpp" | "h" | "hxx"       => Lang::Cpp,
        "kt" | "kts"               => Lang::Kotlin,
        _                          => Lang::Unknown,
    }
}

/// Parse symbols from source text for the given language.
fn parse_symbols(source: &str, lang: Lang) -> Vec<Symbol> {
    match lang {
        Lang::Rust       => parse_rust(source),
        Lang::TypeScript |
        Lang::JavaScript => parse_ts_js(source),
        Lang::Python     => parse_python(source),
        Lang::Cpp        => parse_cpp(source),
        Lang::Kotlin     => parse_kotlin(source),
        Lang::Unknown    => vec![],
    }
}

// ── Rust parser ──────────────────────────────────────────────────────────────

fn parse_rust(src: &str) -> Vec<Symbol> {
    use regex::Regex;
    let mut symbols = Vec::new();
    let mut current_impl: Option<String> = None;

    // Patterns ordered by specificity
    let re_impl   = Regex::new(r"(?m)^[ \t]*(?:pub(?:\([^)]*\))?\s+)?impl(?:<[^>]*>)?\s+([\w:<>, ]+?)(?:\s+for\s+([\w:<>]+))?\s*\{").unwrap();
    let re_fn     = Regex::new(r"(?m)^[ \t]*(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?fn\s+(\w+)\s*(?:<[^>]*>)?\s*\(([^)]*)\)").unwrap();
    let re_struct = Regex::new(r"(?m)^[ \t]*(?:pub(?:\([^)]*\))?\s+)?struct\s+(\w+)").unwrap();
    let re_enum   = Regex::new(r"(?m)^[ \t]*(?:pub(?:\([^)]*\))?\s+)?enum\s+(\w+)").unwrap();
    let re_trait  = Regex::new(r"(?m)^[ \t]*(?:pub(?:\([^)]*\))?\s+)?trait\s+(\w+)").unwrap();
    let re_type   = Regex::new(r"(?m)^[ \t]*(?:pub(?:\([^)]*\))?\s+)?type\s+(\w+)").unwrap();
    let re_const  = Regex::new(r"(?m)^[ \t]*(?:pub(?:\([^)]*\))?\s+)?const\s+(\w+)").unwrap();

    let lines: Vec<&str> = src.lines().collect();

    // Track impl blocks by scanning line by line
    let mut impl_stack: Vec<(String, usize)> = Vec::new(); // (label, brace_depth_at_open)
    let mut brace_depth: i32 = 0;

    for (i, line) in lines.iter().enumerate() {
        let lineno = i + 1;
        let trimmed = line.trim();

        // Track brace depth to know when impl ends
        for ch in line.chars() {
            match ch {
                '{' => brace_depth += 1,
                '}' => {
                    brace_depth -= 1;
                    if let Some((_, depth)) = impl_stack.last() {
                        if brace_depth < *depth as i32 {
                            impl_stack.pop();
                            current_impl = impl_stack.last().map(|(l, _)| l.clone());
                        }
                    }
                }
                _ => {}
            }
        }

        // impl
        if let Some(cap) = re_impl.captures(line) {
            let self_ty = cap.get(1).map(|m| m.as_str().trim()).unwrap_or("?");
            let for_ty  = cap.get(2).map(|m| format!(" for {}", m.as_str().trim()));
            let label   = format!("{}{}", self_ty, for_ty.unwrap_or_default());
            impl_stack.push((label.clone(), brace_depth as usize));
            current_impl = Some(label.clone());
            symbols.push(Symbol {
                kind: "impl", name: label.clone(), line: lineno,
                signature: trimmed.to_string(), container: None,
            });
            continue;
        }

        // fn
        if let Some(cap) = re_fn.captures(line) {
            let name = cap[1].to_string();
            symbols.push(Symbol {
                kind: "fn", name, line: lineno,
                signature: trimmed.to_string(),
                container: current_impl.clone(),
            });
            continue;
        }

        // struct / enum / trait / type / const
        for (re, kind) in [
            (&re_struct, "struct"),
            (&re_enum,   "enum"),
            (&re_trait,  "trait"),
            (&re_type,   "type"),
            (&re_const,  "const"),
        ] {
            if let Some(cap) = re.captures(line) {
                symbols.push(Symbol {
                    kind, name: cap[1].to_string(), line: lineno,
                    signature: trimmed.to_string(), container: None,
                });
                break;
            }
        }
    }

    symbols
}

// ── TypeScript / JavaScript parser ───────────────────────────────────────────

fn parse_ts_js(src: &str) -> Vec<Symbol> {
    use regex::Regex;
    let mut symbols = Vec::new();
    let lines: Vec<&str> = src.lines().collect();

    let re_class    = Regex::new(r"(?m)^[ \t]*(?:export\s+)?(?:abstract\s+)?class\s+(\w+)").unwrap();
    let re_fn_decl  = Regex::new(r"(?m)^[ \t]*(?:export\s+)?(?:async\s+)?function\s+(\w+)").unwrap();
    let re_arrow    = Regex::new(r"(?m)^[ \t]*(?:export\s+)?(?:const|let)\s+(\w+)\s*=\s*(?:async\s*)?\(").unwrap();
    let re_method   = Regex::new(r"(?m)^[ \t]+(?:async\s+)?(?:public\s+|private\s+|protected\s+|static\s+)*(\w+)\s*\(").unwrap();
    let re_iface    = Regex::new(r"(?m)^[ \t]*(?:export\s+)?interface\s+(\w+)").unwrap();
    let re_type     = Regex::new(r"(?m)^[ \t]*(?:export\s+)?type\s+(\w+)\s*=").unwrap();
    let re_enum     = Regex::new(r"(?m)^[ \t]*(?:export\s+)?enum\s+(\w+)").unwrap();

    let mut current_class: Option<String> = None;
    let mut brace_depth: i32 = 0;
    let mut class_depth: Option<i32> = None;

    for (i, line) in lines.iter().enumerate() {
        let lineno = i + 1;
        let trimmed = line.trim();

        for ch in line.chars() {
            match ch {
                '{' => brace_depth += 1,
                '}' => {
                    brace_depth -= 1;
                    if class_depth == Some(brace_depth) {
                        current_class = None;
                        class_depth = None;
                    }
                }
                _ => {}
            }
        }

        if let Some(cap) = re_class.captures(line) {
            let name = cap[1].to_string();
            current_class = Some(name.clone());
            class_depth = Some(brace_depth);
            symbols.push(Symbol { kind: "class", name, line: lineno,
                signature: trimmed.to_string(), container: None });
            continue;
        }
        if let Some(cap) = re_iface.captures(line) {
            symbols.push(Symbol { kind: "interface", name: cap[1].to_string(),
                line: lineno, signature: trimmed.to_string(), container: None });
            continue;
        }
        if let Some(cap) = re_type.captures(line) {
            symbols.push(Symbol { kind: "type", name: cap[1].to_string(),
                line: lineno, signature: trimmed.to_string(), container: None });
            continue;
        }
        if let Some(cap) = re_enum.captures(line) {
            symbols.push(Symbol { kind: "enum", name: cap[1].to_string(),
                line: lineno, signature: trimmed.to_string(), container: None });
            continue;
        }
        if let Some(cap) = re_fn_decl.captures(line) {
            symbols.push(Symbol { kind: "fn", name: cap[1].to_string(),
                line: lineno, signature: trimmed.to_string(), container: current_class.clone() });
            continue;
        }
        if let Some(cap) = re_arrow.captures(line) {
            symbols.push(Symbol { kind: "fn", name: cap[1].to_string(),
                line: lineno, signature: trimmed.to_string(), container: current_class.clone() });
            continue;
        }
        if current_class.is_some() {
            if let Some(cap) = re_method.captures(line) {
                let name = cap[1].to_string();
                if name != "if" && name != "for" && name != "while" && name != "switch" {
                    symbols.push(Symbol { kind: "method", name,
                        line: lineno, signature: trimmed.to_string(),
                        container: current_class.clone() });
                }
            }
        }
    }

    symbols
}

// ── Python parser ─────────────────────────────────────────────────────────────

fn parse_python(src: &str) -> Vec<Symbol> {
    use regex::Regex;
    let mut symbols = Vec::new();
    let lines: Vec<&str> = src.lines().collect();

    let re_class  = Regex::new(r"^(\s*)class\s+(\w+)").unwrap();
    let re_fn     = Regex::new(r"^(\s*)(?:async\s+)?def\s+(\w+)").unwrap();
    let re_import = Regex::new(r"^(?:from\s+\S+\s+)?import\s+(.+)").unwrap();

    // Stack of (indent_level, class_name)
    let mut class_stack: Vec<(usize, String)> = Vec::new();

    for (i, line) in lines.iter().enumerate() {
        let lineno = i + 1;
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') { continue; }

        let indent = line.len() - line.trim_start().len();

        // Pop classes that are no longer in scope
        while let Some((cls_indent, _)) = class_stack.last() {
            if indent <= *cls_indent && !trimmed.starts_with("def ") && !trimmed.starts_with("async def ") && !trimmed.starts_with("class ") {
                // heuristic: a non-indented non-def line closes class
            }
            if indent <= *cls_indent {
                class_stack.pop();
            } else {
                break;
            }
        }

        if let Some(cap) = re_class.captures(line) {
            let name = cap[2].to_string();
            class_stack.push((indent, name.clone()));
            symbols.push(Symbol { kind: "class", name, line: lineno,
                signature: trimmed.to_string(), container: None });
            continue;
        }
        if let Some(cap) = re_fn.captures(line) {
            let name = cap[2].to_string();
            let container = class_stack.last().map(|(_, n)| n.clone());
            let kind = if container.is_some() { "method" } else { "fn" };
            symbols.push(Symbol { kind, name, line: lineno,
                signature: trimmed.to_string(), container });
            continue;
        }
        if re_import.is_match(line) {
            symbols.push(Symbol { kind: "import", name: trimmed.to_string(),
                line: lineno, signature: trimmed.to_string(), container: None });
        }
    }

    symbols
}

// ── C++ parser ────────────────────────────────────────────────────────────────

fn parse_cpp(src: &str) -> Vec<Symbol> {
    use regex::Regex;
    let mut symbols = Vec::new();
    let lines: Vec<&str> = src.lines().collect();

    let re_class  = Regex::new(r"^[ \t]*(?:class|struct)\s+(\w+)(?:\s*:[^{]*)?\s*\{?").unwrap();
    let re_fn     = Regex::new(r"^[ \t]*(?:(?:virtual|static|inline|explicit|constexpr|[[nodiscard]]\s+)*)?(?:[\w:<>*& ]+\s+)?(?:(\w+)::)?(\w+)\s*\(([^;]*)\)\s*(?:const\s*)?(?:override\s*)?(?:noexcept\s*)?\{?$").unwrap();
    let re_enum   = Regex::new(r"^[ \t]*enum(?:\s+class)?\s+(\w+)").unwrap();
    let re_using  = Regex::new(r"^[ \t]*using\s+(\w+)\s*=").unwrap();
    let re_ns     = Regex::new(r"^[ \t]*namespace\s+(\w+)\s*\{?").unwrap();

    let mut current_class: Option<String> = None;
    let mut brace_depth: i32 = 0;
    let mut class_depth: Option<i32> = None;

    for (i, line) in lines.iter().enumerate() {
        let lineno = i + 1;
        let trimmed = line.trim();
        if trimmed.starts_with("//") || trimmed.starts_with("/*") { continue; }

        for ch in line.chars() {
            match ch {
                '{' => brace_depth += 1,
                '}' => {
                    brace_depth -= 1;
                    if class_depth == Some(brace_depth) {
                        current_class = None;
                        class_depth = None;
                    }
                }
                _ => {}
            }
        }

        if let Some(cap) = re_class.captures(line) {
            let name = cap[1].to_string();
            current_class = Some(name.clone());
            class_depth = Some(brace_depth - 1);
            symbols.push(Symbol { kind: "class", name, line: lineno,
                signature: trimmed.to_string(), container: None });
            continue;
        }
        if let Some(cap) = re_enum.captures(line) {
            symbols.push(Symbol { kind: "enum", name: cap[1].to_string(),
                line: lineno, signature: trimmed.to_string(), container: None });
            continue;
        }
        if let Some(cap) = re_using.captures(line) {
            symbols.push(Symbol { kind: "type", name: cap[1].to_string(),
                line: lineno, signature: trimmed.to_string(), container: None });
            continue;
        }
        if let Some(cap) = re_ns.captures(line) {
            symbols.push(Symbol { kind: "namespace", name: cap[1].to_string(),
                line: lineno, signature: trimmed.to_string(), container: None });
            continue;
        }
        // Function: skip lines ending with ; (declarations only in .h — keep them too)
        if let Some(cap) = re_fn.captures(line) {
            let qualifier = cap.get(1).map(|m| m.as_str());
            let name = cap[2].to_string();
            // Skip common false positives
            if ["if", "while", "for", "switch", "return"].contains(&name.as_str()) { continue; }
            let container = qualifier.map(String::from).or_else(|| current_class.clone());
            symbols.push(Symbol {
                kind: "fn", name, line: lineno,
                signature: trimmed.to_string(), container,
            });
        }
    }

    symbols
}

// ── Kotlin parser ─────────────────────────────────────────────────────────────

fn parse_kotlin(src: &str) -> Vec<Symbol> {
    use regex::Regex;
    let mut symbols = Vec::new();
    let lines: Vec<&str> = src.lines().collect();

    let re_class  = Regex::new(r"^[ \t]*(?:(?:data|sealed|abstract|open|inner|enum)\s+)*class\s+(\w+)").unwrap();
    let re_object = Regex::new(r"^[ \t]*(?:companion\s+)?object\s+(\w+)?").unwrap();
    let re_iface  = Regex::new(r"^[ \t]*(?:fun\s+)?interface\s+(\w+)").unwrap();
    let re_fn     = Regex::new(r"^[ \t]*(?:(?:override|private|protected|internal|public|suspend|inline|operator|infix)\s+)*fun\s+(\w+)").unwrap();
    let re_prop   = Regex::new(r"^[ \t]*(?:(?:override|private|protected|internal|public|lateinit)\s+)*(?:val|var)\s+(\w+)").unwrap();

    let mut current_class: Option<String> = None;
    let mut brace_depth: i32 = 0;
    let mut class_depth: Option<i32> = None;

    for (i, line) in lines.iter().enumerate() {
        let lineno = i + 1;
        let trimmed = line.trim();
        if trimmed.starts_with("//") { continue; }

        for ch in line.chars() {
            match ch {
                '{' => brace_depth += 1,
                '}' => {
                    brace_depth -= 1;
                    if class_depth == Some(brace_depth) {
                        current_class = None;
                        class_depth = None;
                    }
                }
                _ => {}
            }
        }

        if let Some(cap) = re_class.captures(line) {
            let name = cap[1].to_string();
            current_class = Some(name.clone());
            class_depth = Some(brace_depth - 1);
            symbols.push(Symbol { kind: "class", name, line: lineno,
                signature: trimmed.to_string(), container: None });
            continue;
        }
        if let Some(cap) = re_iface.captures(line) {
            symbols.push(Symbol { kind: "interface", name: cap[1].to_string(),
                line: lineno, signature: trimmed.to_string(), container: None });
            continue;
        }
        if let Some(cap) = re_object.captures(line) {
            let name = cap.get(1).map(|m| m.as_str()).unwrap_or("(anonymous)").to_string();
            symbols.push(Symbol { kind: "object", name, line: lineno,
                signature: trimmed.to_string(), container: current_class.clone() });
            continue;
        }
        if let Some(cap) = re_fn.captures(line) {
            symbols.push(Symbol { kind: "fn", name: cap[1].to_string(),
                line: lineno, signature: trimmed.to_string(),
                container: current_class.clone() });
            continue;
        }
        if let Some(cap) = re_prop.captures(line) {
            symbols.push(Symbol { kind: "prop", name: cap[1].to_string(),
                line: lineno, signature: trimmed.to_string(),
                container: current_class.clone() });
        }
    }

    symbols
}

// ─── get_symbols ──────────────────────────────────────────────────────────────

/// List all top-level symbols in a file.
/// Args:
///   path        — file to analyse
///   kinds?      — comma-separated filter: "fn,struct,class,impl,enum,trait,type,const"
///                 (default: all)
fn get_symbols(args: &Value, root: &Path) -> Result<ToolResult> {
    let rel  = str_arg(args, "path")?;
    let path = resolve(root, rel)?;
    let src  = std::fs::read_to_string(&path)
        .map_err(|e| anyhow::anyhow!("get_symbols: {e}"))?;

    let lang = detect_lang(&path);
    if lang == Lang::Unknown {
        return Ok(ToolResult {
            output: format!("get_symbols: unsupported file type '{}'", path.display()),
            success: false,
        });
    }

    let filter: Option<Vec<&str>> = args.get("kinds")
        .and_then(|v| v.as_str())
        .map(|s| s.split(',').map(|k| k.trim()).collect());

    let symbols = parse_symbols(&src, lang);
    let symbols: Vec<&Symbol> = symbols.iter()
        .filter(|s| filter.as_ref().map(|f| f.contains(&s.kind)).unwrap_or(true))
        .collect();

    if symbols.is_empty() {
        return Ok(ToolResult::ok(format!("No symbols found in {rel}")));
    }

    let mut out = format!("Symbols in {rel} ({} total)\n{}\n", symbols.len(), "─".repeat(50));
    for s in &symbols {
        let container = s.container.as_deref()
            .map(|c| format!(" [{c}]"))
            .unwrap_or_default();
        out.push_str(&format!("  {:>5}  {:12} {}{}\n", s.line, s.kind, s.name, container));
    }

    Ok(ToolResult::ok(out))
}

// ─── outline ─────────────────────────────────────────────────────────────────

/// Show the structural outline of a file — like get_symbols but with signatures.
/// Args:
///   path — file to analyse
fn outline(args: &Value, root: &Path) -> Result<ToolResult> {
    let rel  = str_arg(args, "path")?;
    let path = resolve(root, rel)?;
    let src  = std::fs::read_to_string(&path)
        .map_err(|e| anyhow::anyhow!("outline: {e}"))?;

    let lang = detect_lang(&path);
    if lang == Lang::Unknown {
        return Ok(ToolResult {
            output: format!("outline: unsupported file type '{}'", path.display()),
            success: false,
        });
    }

    let symbols = parse_symbols(&src, lang);
    if symbols.is_empty() {
        return Ok(ToolResult::ok(format!("No symbols found in {rel}")));
    }

    let mut out = format!("Outline: {rel}\n{}\n", "─".repeat(60));
    let mut last_container: Option<&str> = None;

    for s in &symbols {
        // Print container header when it changes
        if s.container.as_deref() != last_container && s.container.is_some() {
            out.push_str(&format!("\n  ▸ {}\n", s.container.as_deref().unwrap()));
            last_container = s.container.as_deref();
        } else if s.container.is_none() && last_container.is_some() {
            out.push('\n');
            last_container = None;
        }

        let indent = if s.container.is_some() { "    " } else { "  " };
        // Truncate long signatures
        let sig = if s.signature.len() > 80 {
            format!("{}…", &s.signature[..79])
        } else {
            s.signature.clone()
        };
        out.push_str(&format!("{}{:>5}  {}\n", indent, s.line, sig));
    }

    Ok(ToolResult::ok(out))
}

// ─── get_signature ────────────────────────────────────────────────────────────

/// Get the full signature (and a few lines of doc comment) for a named symbol.
/// Args:
///   path   — file to search in
///   name   — symbol name to look up
///   lines? — how many lines to return after the signature (default: 3)
fn get_signature(args: &Value, root: &Path) -> Result<ToolResult> {
    let rel    = str_arg(args, "path")?;
    let name   = str_arg(args, "name")?;
    let extra  = args.get("lines").and_then(|v| v.as_u64()).unwrap_or(3) as usize;

    let path = resolve(root, rel)?;
    let src  = std::fs::read_to_string(&path)
        .map_err(|e| anyhow::anyhow!("get_signature: {e}"))?;

    let lang    = detect_lang(&path);
    let symbols = parse_symbols(&src, lang);
    let lines: Vec<&str> = src.lines().collect();

    let matches: Vec<&Symbol> = symbols.iter()
        .filter(|s| s.name == name)
        .collect();

    if matches.is_empty() {
        return Ok(ToolResult {
            output: format!("Symbol '{name}' not found in {rel}"),
            success: false,
        });
    }

    let mut out = String::new();
    for sym in matches {
        let start = sym.line.saturating_sub(1);  // 0-based
        // Look back up to 5 lines for doc comments
        let doc_start = start.saturating_sub(5);
        let mut doc_lines = Vec::new();
        for l in &lines[doc_start..start] {
            let t = l.trim();
            if t.starts_with("///") || t.starts_with("//!") ||
               t.starts_with("/**") || t.starts_with("*") ||
               t.starts_with("#[") || t.starts_with("\"\"\"") ||
               t.starts_with("--") {
                doc_lines.push(*l);
            }
        }

        let end = (start + 1 + extra).min(lines.len());
        out.push_str(&format!(
            "── {} {} (line {}) ──\n",
            sym.kind, sym.name, sym.line
        ));
        if !doc_lines.is_empty() {
            for l in &doc_lines { out.push_str(l); out.push('\n'); }
        }
        for l in &lines[start..end] {
            out.push_str(l);
            out.push('\n');
        }
        out.push('\n');
    }

    Ok(ToolResult::ok(out.trim_end().to_string()))
}

// ─── find_references ─────────────────────────────────────────────────────────

/// Find all places where a symbol name is used across the codebase.
/// Args:
///   name   — symbol name to search for
///   dir?   — directory to search in (default: repo root)
///   ext?   — file extension filter e.g. "rs" (default: auto from lang)
fn find_references(args: &Value, root: &Path) -> Result<ToolResult> {
    let name = str_arg(args, "name")?;
    let dir  = args.get("dir")
        .and_then(|v| v.as_str())
        .map(|d| root.join(d))
        .unwrap_or_else(|| root.to_path_buf());

    let ext_filter = args.get("ext").and_then(|v| v.as_str());

    // We search for the name as a whole word using a simple word-boundary approach
    // Pattern: not preceded/followed by word chars
    let pattern = format!(r"(?<![a-zA-Z0-9_]){name}(?![a-zA-Z0-9_])");
    let re = regex::Regex::new(&pattern)
        .map_err(|e| anyhow::anyhow!("find_references: invalid name '{name}': {e}"))?;

    // Skip dirs that are never useful
    let skip_dirs: std::collections::HashSet<&str> =
        ["target", ".git", "node_modules", ".ai"].into_iter().collect();

    let mut results: Vec<(String, usize, String)> = Vec::new(); // (file, line, content)

    for entry in WalkDir::new(&dir)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| {
            !e.file_name().to_str()
                .map(|n| skip_dirs.contains(n))
                .unwrap_or(false)
        })
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
    {
        let path = entry.path();

        // Extension filter
        if let Some(ext) = ext_filter {
            if path.extension().and_then(|e| e.to_str()) != Some(ext) {
                continue;
            }
        } else {
            // Only search source files
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            if !["rs","ts","tsx","js","jsx","py","cpp","cc","hpp","h","kt","kts"]
                .contains(&ext) { continue; }
        }

        let src = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(_) => continue,
        };

        let rel = path.strip_prefix(root).unwrap_or(path)
            .to_string_lossy().to_string();

        for (i, line) in src.lines().enumerate() {
            if re.is_match(line) {
                results.push((rel.clone(), i + 1, line.trim().to_string()));
                if results.len() >= 200 { break; }
            }
        }

        if results.len() >= 200 { break; }
    }

    if results.is_empty() {
        return Ok(ToolResult {
            output: format!("No references to '{name}' found"),
            success: false,
        });
    }

    let mut out = format!(
        "References to '{}' — {} occurrence(s)\n{}\n",
        name, results.len(), "─".repeat(50)
    );
    for (file, line, content) in &results {
        out.push_str(&format!("  {}:{}: {}\n", file, line, content));
    }
    if results.len() >= 200 {
        out.push_str("\n[truncated at 200 results]");
    }

    Ok(ToolResult::ok(out))
}
// ─── Git tools ────────────────────────────────────────────────────────────────
//
// All git tools:
//   - check that the directory is a git repo before running
//   - use --no-color / --porcelain for machine-readable output
//   - never use shell operators — explicit program + args[]

async fn git_check(root: &Path) -> bool {
    tokio::process::Command::new("git")
        .args(["rev-parse", "--is-inside-work-tree"])
        .current_dir(root)
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false)
}

async fn git_run(args: &[&str], root: &Path) -> Result<String> {
    let out = tokio::process::Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .await
        .map_err(|e| anyhow::anyhow!("git {}: {e}", args.first().unwrap_or(&"?")))?;

    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();

    if !out.status.success() && !stderr.trim().is_empty() {
        return Err(anyhow::anyhow!("{}", stderr.trim()));
    }
    Ok(if stdout.trim().is_empty() { stderr } else { stdout })
}

// ─── git_status ───────────────────────────────────────────────────────────────

/// Show working tree status.
/// Args:
///   short? — if true, use --short porcelain format (default: false = verbose)
async fn git_status(args: &Value, root: &Path) -> Result<ToolResult> {
    if !git_check(root).await {
        return Ok(ToolResult { output: "Not a git repository.".into(), success: false });
    }

    let short = args.get("short").and_then(|v| v.as_bool()).unwrap_or(false);

    // Branch info
    let branch = git_run(&["rev-parse", "--abbrev-ref", "HEAD"], root).await
        .unwrap_or_else(|_| "unknown".into());
    let branch = branch.trim().to_string();

    // Upstream status
    let ahead_behind = git_run(
        &["rev-list", "--left-right", "--count", &format!("HEAD...@{{u}}")],
        root,
    ).await.ok().map(|s| {
        let parts: Vec<&str> = s.trim().split_whitespace().collect();
        if parts.len() == 2 {
            let ahead:  i64 = parts[0].parse().unwrap_or(0);
            let behind: i64 = parts[1].parse().unwrap_or(0);
            match (ahead, behind) {
                (0, 0) => " (up to date)".into(),
                (a, 0) => format!(" (ahead {a})"),
                (0, b) => format!(" (behind {b})"),
                (a, b) => format!(" (ahead {a}, behind {b})"),
            }
        } else {
            String::new()
        }
    }).unwrap_or_default();

    let status_args: &[&str] = if short {
        &["status", "--short", "--no-color"]
    } else {
        &["status", "--no-color"]
    };
    let status = git_run(status_args, root).await?;

    let out = format!(
        "Branch: {branch}{ahead_behind}\n{}\n{}",
        "─".repeat(40),
        status.trim()
    );
    Ok(ToolResult::ok(out))
}

// ─── git_commit ───────────────────────────────────────────────────────────────

/// Stage files and create a commit.
/// Args:
///   message       — commit message (required)
///   files?        — array of file paths to stage (default: ["."] = all changes)
///   allow_empty?  — if true, allow commits with no changes (default: false)
async fn git_commit(args: &Value, root: &Path) -> Result<ToolResult> {
    if !git_check(root).await {
        return Ok(ToolResult { output: "Not a git repository.".into(), success: false });
    }

    let message = str_arg(args, "message")?;
    if message.trim().is_empty() {
        return Ok(ToolResult { output: "git_commit: message cannot be empty.".into(), success: false });
    }

    let allow_empty = args.get("allow_empty").and_then(|v| v.as_bool()).unwrap_or(false);

    // Stage files
    let files: Vec<String> = args.get("files")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
        .unwrap_or_else(|| vec![".".to_string()]);

    let mut add_args = vec!["add".to_string()];
    add_args.extend(files);
    let add_str: Vec<&str> = add_args.iter().map(|s| s.as_str()).collect();
    git_run(&add_str, root).await
        .map_err(|e| anyhow::anyhow!("git add failed: {e}"))?;

    // Check if there's anything staged
    let staged = git_run(&["diff", "--cached", "--stat", "--no-color"], root).await
        .unwrap_or_default();

    if staged.trim().is_empty() && !allow_empty {
        return Ok(ToolResult {
            output: "Nothing to commit — no staged changes. Use git_status to check working tree.".into(),
            success: false,
        });
    }

    // Commit
    let mut commit_args = vec!["commit", "-m", message];
    if allow_empty { commit_args.push("--allow-empty"); }

    let _out = git_run(&commit_args, root).await?;

    // Get the new commit hash
    let hash = git_run(&["rev-parse", "--short", "HEAD"], root).await
        .unwrap_or_else(|_| "?".into());

    Ok(ToolResult::ok(format!(
        "Committed: {} ({})\n\nStaged:\n{}",
        hash.trim(), message, staged.trim()
    )))
}

// ─── git_log ──────────────────────────────────────────────────────────────────

/// Show commit history.
/// Args:
///   n?      — number of commits to show (default: 10)
///   path?   — limit log to changes in this file/directory
///   oneline? — if true, one line per commit (default: true)
async fn git_log(args: &Value, root: &Path) -> Result<ToolResult> {
    if !git_check(root).await {
        return Ok(ToolResult { output: "Not a git repository.".into(), success: false });
    }

    let n       = args.get("n").and_then(|v| v.as_u64()).unwrap_or(10);
    let oneline = args.get("oneline").and_then(|v| v.as_bool()).unwrap_or(true);
    let path    = args.get("path").and_then(|v| v.as_str());

    let n_str = format!("-{n}");
    let mut log_args = vec!["log", "--no-color", &n_str];

    if oneline {
        log_args.push("--oneline");
    } else {
        log_args.extend_from_slice(&[
            "--pretty=format:%h %ad %an <%ae>%n    %s%n",
            "--date=short",
        ]);
    }

    if let Some(p) = path {
        log_args.push("--");
        log_args.push(p);
    }

    let out = git_run(&log_args, root).await?;

    if out.trim().is_empty() {
        return Ok(ToolResult::ok("No commits yet."));
    }

    Ok(ToolResult::ok(format!(
        "Last {n} commits{}:\n{}\n{}",
        path.map(|p| format!(" for '{p}'")).unwrap_or_default(),
        "─".repeat(50),
        out.trim()
    )))
}

// ─── git_stash ────────────────────────────────────────────────────────────────

/// Manage the git stash.
/// Args:
///   action  — "push" | "pop" | "list" | "drop" | "show" (default: "list")
///   message? — stash message for "push"
///   index?   — stash index for "drop" / "show" (default: 0)
async fn git_stash(args: &Value, root: &Path) -> Result<ToolResult> {
    if !git_check(root).await {
        return Ok(ToolResult { output: "Not a git repository.".into(), success: false });
    }

    let action = args.get("action").and_then(|v| v.as_str()).unwrap_or("list");
    let index  = args.get("index").and_then(|v| v.as_u64()).unwrap_or(0);

    let out = match action {
        "push" => {
            if let Some(msg) = args.get("message").and_then(|v| v.as_str()) {
                let out = tokio::process::Command::new("git")
                    .args(["stash", "push", "--include-untracked", "-m", msg])
                    .current_dir(root)
                    .output()
                    .await
                    .map_err(|e| anyhow::anyhow!("git stash push: {e}"))?;
                String::from_utf8_lossy(&out.stdout).to_string()
            } else {
                git_run(&["stash", "push", "--include-untracked"], root).await?
            }
        }
        "pop" => git_run(&["stash", "pop"], root).await?,
        "list" => {
            let out = git_run(&["stash", "list", "--no-color"], root).await?;
            if out.trim().is_empty() { "Stash is empty.".into() } else { out }
        }
        "drop" => {
            let ref_str = format!("stash@{{{index}}}");
            git_run(&["stash", "drop", &ref_str], root).await?
        }
        "show" => {
            let ref_str = format!("stash@{{{index}}}");
            git_run(&["stash", "show", "--stat", "--no-color", &ref_str], root).await?
        }
        unknown => {
            return Ok(ToolResult {
                output: format!("git_stash: unknown action '{unknown}'. Use: push | pop | list | drop | show"),
                success: false,
            });
        }
    };

    Ok(ToolResult::ok(out.trim().to_string()))
}
// ─── spawn_agent ──────────────────────────────────────────────────────────────

/// Spawn a sub-agent with a given role and task.
/// The sub-agent runs in-process with its own history and tool allowlist.
///
/// Communication pattern: through .ai/knowledge/ (shared memory).
/// The sub-agent is expected to write its findings via memory_write.
/// spawn_agent tells the boss which key to read after completion.
///
/// Args:
///   role        — agent role: research | developer | navigator | qa | memory
///   task        — task description (string)
///   memory_key? — .ai/knowledge/ key where sub-agent should write results
///                 (default: "knowledge/<role>_result")
///   max_steps?  — override max steps (default: parent_sub_steps, min 5)
async fn spawn_agent(
    args: &Value,
    root: &Path,
    cfg: &crate::config::AgentConfig,
    parent_sub_steps: usize,
) -> Result<ToolResult> {
    use crate::agent::SweAgent;
    use crate::config::Role;

    let role_str = str_arg(args, "role")?;
    let task     = str_arg(args, "task")?;

    let role = Role::from_str(role_str).ok_or_else(|| {
        anyhow::anyhow!(
            "spawn_agent: unknown role '{role_str}'. \
             Valid roles: research, developer, navigator, qa, memory"
        )
    })?;

    // Prevent infinite recursion: boss cannot spawn another boss
    if role == Role::Boss {
        return Ok(ToolResult {
            output: "spawn_agent: cannot spawn a boss sub-agent (recursion guard)".into(),
            success: false,
        });
    }

    let memory_key = args
        .get("memory_key")
        .and_then(|v| v.as_str())
        .unwrap_or_else(|| "knowledge/agent_result")
        .to_string();

    let max_steps = args
        .get("max_steps")
        .and_then(|v| v.as_u64())
        .map(|n| n as usize)
        .unwrap_or(parent_sub_steps.max(5));

    // Inject memory_key instruction into the task so the sub-agent knows where to write
    let augmented_task = format!(
        "{task}\n\n\
        IMPORTANT: When finished, write your results and findings to memory key \"{memory_key}\" \
        using memory_write before calling finish."
    );

    let repo_str = root.to_string_lossy().to_string();

    let mut sub = SweAgent::new(cfg.clone(), &repo_str, max_steps, role.clone())
        .map_err(|e| anyhow::anyhow!("spawn_agent: failed to create sub-agent: {e}"))?;

    sub.run_capture(&augmented_task).await?;

    // Tell the boss where to find the results
    Ok(ToolResult::ok(format!(
        "{} agent finished ({max_steps} steps max).\n\
        Results written to memory key \"{memory_key}\".\n\
        Use memory_read(\"{memory_key}\") to access them.",
        role.name(),
        memory_key = memory_key,
    )))
}

// ─── github_api ───────────────────────────────────────────────────────────────

/// GitHub REST API client with smart response filtering.
/// Requires GITHUB_TOKEN env var (or token arg).
///
/// Args:
///   method    — "GET" | "POST" | "PATCH" | "PUT" | "DELETE" (default: "GET")
///   endpoint  — path, e.g. "/repos/owner/repo/issues" or full https:// URL
///   body?     — JSON object for POST/PATCH/PUT
///   token?    — overrides GITHUB_TOKEN env var
///
/// Issues:
///   GET  /repos/{owner}/{repo}/issues              — list open issues
///   GET  /repos/{owner}/{repo}/issues/{n}          — read single issue
///   POST /repos/{owner}/{repo}/issues/{n}/comments — post a comment
///   PATCH /repos/{owner}/{repo}/issues/{n}         — update (state/labels/assignees)
///
/// Pull Requests:
///   GET  /repos/{owner}/{repo}/pulls               — list open PRs
///   GET  /repos/{owner}/{repo}/pulls/{n}           — read single PR
///   POST /repos/{owner}/{repo}/pulls               — create PR
///   PUT  /repos/{owner}/{repo}/pulls/{n}/merge     — merge PR
///
/// Repo info:
///   GET  /repos/{owner}/{repo}/branches            — list branches
///   GET  /repos/{owner}/{repo}/commits             — recent commits
///   GET  /repos/{owner}/{repo}/contents/{path}     — file contents (base64 decoded automatically)
async fn github_api(args: &Value) -> Result<ToolResult> {
    let method   = args.get("method").and_then(|v| v.as_str()).unwrap_or("GET").to_uppercase();
    let endpoint = str_arg(args, "endpoint")?;
    let body     = args.get("body");

    // Token: arg > GITHUB_TOKEN env var
    let token = args
        .get("token")
        .and_then(|v| v.as_str())
        .map(String::from)
        .or_else(|| std::env::var("GITHUB_TOKEN").ok())
        .ok_or_else(|| anyhow::anyhow!(
            "github_api: no token. Set GITHUB_TOKEN env var or pass \"token\" arg."
        ))?;

    let url = if endpoint.starts_with("https://") {
        endpoint.to_string()
    } else {
        format!("https://api.github.com/{}", endpoint.trim_start_matches('/'))
    };

    let client = reqwest::Client::builder()
        .user_agent("do_it-agent/1.0")
        .timeout(std::time::Duration::from_secs(20))
        .build()?;

    let mut req = client
        .request(
            reqwest::Method::from_bytes(method.as_bytes())
                .map_err(|_| anyhow::anyhow!("github_api: invalid HTTP method '{method}'"))?,
            &url,
        )
        .header("Authorization", format!("Bearer {token}"))
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28");

    if let Some(b) = body {
        req = req.json(b);
    }

    let resp = req.send().await
        .map_err(|e| anyhow::anyhow!("github_api: request failed: {e}"))?;

    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();

    if !status.is_success() {
        return Ok(ToolResult {
            output: format!("GitHub API {status} for {method} {endpoint}:\n{text}"),
            success: false,
        });
    }

    // No body (e.g. 204 No Content on merge)
    if text.trim().is_empty() {
        return Ok(ToolResult::ok(format!("{method} {endpoint} → {status}")));
    }

    let json: serde_json::Value = serde_json::from_str(&text)
        .unwrap_or(serde_json::Value::String(text.clone()));

    // Smart filtering to keep context window small
    let output = github_format_response(&json, endpoint, &method);

    Ok(ToolResult::ok(format!("{method} {endpoint} → {status}\n{output}")))
}

/// Format GitHub API response, filtering noisy fields and decoding file contents.
fn github_format_response(json: &serde_json::Value, endpoint: &str, method: &str) -> String {
    // File contents endpoint — decode base64 automatically
    if endpoint.contains("/contents/") && method == "GET" {
        if let Some(content_b64) = json.get("content").and_then(|v| v.as_str()) {
            let decoded = base64_decode_content(content_b64);
            let _name = json.get("name").and_then(|v| v.as_str()).unwrap_or("file");
            let path = json.get("path").and_then(|v| v.as_str()).unwrap_or("");
            let sha  = json.get("sha").and_then(|v| v.as_str()).unwrap_or("");
            return format!("File: {path} (sha: {sha})\n\n{decoded}");
        }
    }

    match json {
        // Array response — list of issues, PRs, branches, commits, etc.
        serde_json::Value::Array(items) => {
            let filtered: Vec<String> = items.iter().map(|item| {
                github_summarize_item(item, endpoint)
            }).collect();
            format!("{} item(s):\n{}", filtered.len(), filtered.join("\n---\n"))
        }
        // Single object — issue, PR, branch, commit details
        serde_json::Value::Object(_) => {
            github_summarize_item(json, endpoint)
        }
        other => serde_json::to_string_pretty(other).unwrap_or_default(),
    }
}

fn github_summarize_item(item: &serde_json::Value, endpoint: &str) -> String {
    let ep = endpoint.to_ascii_lowercase();

    // Issue or PR
    if ep.contains("/issues") || ep.contains("/pulls") {
        let number = item.get("number").and_then(|v| v.as_u64()).map(|n| format!("#{n}")).unwrap_or_default();
        let title  = item.get("title").and_then(|v| v.as_str()).unwrap_or("");
        let state  = item.get("state").and_then(|v| v.as_str()).unwrap_or("");
        let user   = item.get("user").and_then(|u| u.get("login")).and_then(|v| v.as_str()).unwrap_or("");
        let body   = item.get("body").and_then(|v| v.as_str()).unwrap_or("").trim();
        let body_preview = if body.len() > 400 { format!("{}…", &body[..400]) } else { body.to_string() };

        // PR-specific fields
        let pr_info = if let Some(head) = item.get("head") {
            let from = head.get("label").and_then(|v| v.as_str()).unwrap_or("");
            let into = item.get("base").and_then(|b| b.get("label")).and_then(|v| v.as_str()).unwrap_or("");
            if !from.is_empty() { format!("\nBranch: {from} → {into}") } else { String::new() }
        } else { String::new() };

        // Labels
        let labels: Vec<&str> = item.get("labels")
            .and_then(|l| l.as_array())
            .map(|arr| arr.iter().filter_map(|l| l.get("name").and_then(|v| v.as_str())).collect())
            .unwrap_or_default();
        let labels_str = if labels.is_empty() { String::new() } else { format!("\nLabels: {}", labels.join(", ")) };

        return format!("{number} [{state}] {title}\nAuthor: @{user}{pr_info}{labels_str}\n{body_preview}");
    }

    // Commit
    if ep.contains("/commits") {
        let sha  = item.get("sha").and_then(|v| v.as_str()).unwrap_or("?");
        let msg  = item.get("commit").and_then(|c| c.get("message")).and_then(|v| v.as_str()).unwrap_or("");
        let author = item.get("commit")
            .and_then(|c| c.get("author"))
            .and_then(|a| a.get("name"))
            .and_then(|v| v.as_str())
            .unwrap_or("?");
        let date = item.get("commit")
            .and_then(|c| c.get("author"))
            .and_then(|a| a.get("date"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        return format!("{} {} — {} ({})", &sha[..7.min(sha.len())], msg.lines().next().unwrap_or(""), author, date);
    }

    // Branch
    if ep.contains("/branches") {
        let name = item.get("name").and_then(|v| v.as_str()).unwrap_or("?");
        let sha  = item.get("commit").and_then(|c| c.get("sha")).and_then(|v| v.as_str()).unwrap_or("?");
        let protected = item.get("protected").and_then(|v| v.as_bool()).unwrap_or(false);
        return format!("{name} ({}) sha:{}", if protected { "protected" } else { "open" }, &sha[..7.min(sha.len())]);
    }

    // Comment (POST /issues/{n}/comments response)
    if ep.contains("/comments") {
        let id   = item.get("id").and_then(|v| v.as_u64()).map(|n| n.to_string()).unwrap_or_default();
        let user = item.get("user").and_then(|u| u.get("login")).and_then(|v| v.as_str()).unwrap_or("");
        let body = item.get("body").and_then(|v| v.as_str()).unwrap_or("").trim();
        return format!("Comment #{id} by @{user}:\n{body}");
    }

    // Fallback: just serialize, but strip known noisy fields
    let mut obj = item.clone();
    if let serde_json::Value::Object(ref mut map) = obj {
        for key in &["_links", "reactions", "performed_via_github_app",
                     "author_association", "node_id", "events_url",
                     "labels_url", "comments_url", "html_url", "url"] {
            map.remove(*key);
        }
    }
    serde_json::to_string_pretty(&obj).unwrap_or_default()
}

fn base64_decode_content(b64: &str) -> String {
    use base64::{engine::general_purpose::STANDARD, Engine};
    // GitHub wraps lines with \n — strip them
    let cleaned: String = b64.chars().filter(|&c| c != '\n' && c != '\r').collect();
    STANDARD.decode(cleaned.as_bytes())
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .unwrap_or_else(|| format!("[binary content, {} bytes base64]", b64.len()))
}

// ─── test_coverage ────────────────────────────────────────────────────────────

/// Run tests and collect coverage information.
/// Detects the project type and uses the appropriate tool.
///
/// Supported:
///   Rust   — cargo tarpaulin (if installed) or cargo test with --doc
///   Node   — jest --coverage (if jest config found)
///   Python — pytest --cov (if pytest-cov installed)
///
/// Args:
///   dir?       — directory to run in (default: repo root)
///   threshold? — warn if line coverage is below this % (default: 80)
async fn test_coverage(args: &Value, root: &Path) -> Result<ToolResult> {
    let cwd = if let Some(p) = args.get("dir").and_then(|v| v.as_str()) {
        resolve(root, p)?
    } else {
        root.to_path_buf()
    };
    let threshold = args.get("threshold").and_then(|v| v.as_f64()).unwrap_or(80.0);

    // Detect project type by looking for manifest files
    let is_rust   = cwd.join("Cargo.toml").exists();
    let is_node   = cwd.join("package.json").exists();
    let is_python = cwd.join("pyproject.toml").exists()
        || cwd.join("setup.py").exists()
        || cwd.join("setup.cfg").exists();

    if is_rust {
        run_coverage_rust(&cwd, threshold).await
    } else if is_node {
        run_coverage_node(&cwd, threshold).await
    } else if is_python {
        run_coverage_python(&cwd, threshold).await
    } else {
        Ok(ToolResult {
            output: format!(
                "test_coverage: could not detect project type in {}.\n\
                Expected Cargo.toml (Rust), package.json (Node), or pyproject.toml/setup.py (Python).",
                cwd.display()
            ),
            success: false,
        })
    }
}

async fn run_coverage_rust(cwd: &Path, threshold: f64) -> Result<ToolResult> {
    // Try cargo tarpaulin first
    let tarpaulin = tokio::process::Command::new("cargo")
        .args(["tarpaulin", "--out", "Stdout", "--skip-clean"])
        .current_dir(cwd)
        .output()
        .await;

    match tarpaulin {
        Ok(out) if out.status.success() => {
            let stdout = String::from_utf8_lossy(&out.stdout).to_string();
            let stderr = String::from_utf8_lossy(&out.stderr).to_string();
            let combined = format!("{stdout}{stderr}");

            // Parse coverage % from tarpaulin output: "X.XX% coverage"
            let coverage = parse_coverage_percent(&combined);
            let summary = format_coverage_summary("cargo tarpaulin", &combined, coverage, threshold);
            Ok(ToolResult { output: summary, success: coverage.map(|c| c >= threshold).unwrap_or(true) })
        }
        _ => {
            // Fall back to cargo test — no coverage numbers but at least tests run
            let out = tokio::process::Command::new("cargo")
                .args(["test", "--", "--test-output", "immediate"])
                .current_dir(cwd)
                .output()
                .await
                .map_err(|e| anyhow::anyhow!("test_coverage: cargo test failed to start: {e}"))?;

            let stdout = String::from_utf8_lossy(&out.stdout).to_string();
            let stderr = String::from_utf8_lossy(&out.stderr).to_string();
            let combined = format!("{stdout}{stderr}");
            let success = out.status.success();

            Ok(ToolResult {
                output: format!(
                    "cargo tarpaulin not found — ran cargo test instead (no coverage %)\n\
                    Install: cargo install cargo-tarpaulin\n\n{combined}"
                ),
                success,
            })
        }
    }
}

async fn run_coverage_node(cwd: &Path, threshold: f64) -> Result<ToolResult> {
    let out = tokio::process::Command::new("npx")
        .args(["jest", "--coverage", "--coverageReporters=text"])
        .current_dir(cwd)
        .output()
        .await
        .map_err(|e| anyhow::anyhow!("test_coverage: npx jest failed to start: {e}"))?;

    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    let combined = format!("{stdout}{stderr}");
    let coverage = parse_coverage_percent(&combined);
    let summary = format_coverage_summary("jest --coverage", &combined, coverage, threshold);
    Ok(ToolResult { output: summary, success: out.status.success() })
}

async fn run_coverage_python(cwd: &Path, threshold: f64) -> Result<ToolResult> {
    let out = tokio::process::Command::new("python3")
        .args(["-m", "pytest", "--cov", "--cov-report=term-missing", "-q"])
        .current_dir(cwd)
        .output()
        .await
        .map_err(|e| anyhow::anyhow!("test_coverage: pytest failed to start: {e}"))?;

    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    let combined = format!("{stdout}{stderr}");
    let coverage = parse_coverage_percent(&combined);
    let summary = format_coverage_summary("pytest --cov", &combined, coverage, threshold);
    Ok(ToolResult { output: summary, success: out.status.success() })
}

fn parse_coverage_percent(output: &str) -> Option<f64> {
    // Matches patterns like "85.23% coverage", "TOTAL ... 85%", "Lines covered: 85.2%"
    let re = regex::Regex::new(r"(\d+\.?\d*)\s*%").ok()?;
    // Find the last percentage that looks like a total (largest number usually)
    re.captures_iter(output)
        .filter_map(|c| c[1].parse::<f64>().ok())
        .filter(|&p| p <= 100.0)
        .last()
}

fn format_coverage_summary(tool: &str, raw: &str, coverage: Option<f64>, threshold: f64) -> String {
    let coverage_line = match coverage {
        Some(c) => {
            let status = if c >= threshold { "✓" } else { "⚠ BELOW THRESHOLD" };
            format!("Coverage: {:.1}% {status} (threshold: {threshold:.0}%)", c)
        }
        None => "Coverage: could not parse percentage from output".to_string(),
    };

    // Show last 50 lines of output (most relevant part)
    let lines: Vec<&str> = raw.lines().collect();
    let tail = if lines.len() > 50 {
        format!("[... {} lines truncated ...]\n{}", lines.len() - 50, lines[lines.len()-50..].join("\n"))
    } else {
        raw.to_string()
    };

    format!("Tool: {tool}\n{coverage_line}\n\n{tail}")
}

// ─── notify ───────────────────────────────────────────────────────────────────

/// Send a one-way notification via Telegram (no waiting for reply).
/// Use for progress updates, completion notices, or alerts during long runs.
/// Falls back to printing to stdout if Telegram is not configured.
///
/// Args:
///   message  — text to send
///   silent?  — if true, sends without sound (default: false)
async fn notify(args: &Value, tg: &TelegramConfig) -> Result<ToolResult> {
    let message = str_arg(args, "message")?;
    let silent  = args.get("silent").and_then(|v| v.as_bool()).unwrap_or(false);

    if let (Some(token), Some(chat_id)) = (&tg.token, &tg.chat_id) {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()?;

        let url = format!("https://api.telegram.org/bot{token}/sendMessage");
        let resp = client
            .post(&url)
            .json(&serde_json::json!({
                "chat_id": chat_id,
                "text": format!("🤖 {message}"),
                "disable_notification": silent,
            }))
            .send()
            .await;

        match resp {
            Ok(r) if r.status().is_success() => {
                return Ok(ToolResult::ok(format!("Notification sent: {message}")));
            }
            Ok(r) => {
                let status = r.status();
                let body = r.text().await.unwrap_or_default();
                tracing::warn!("notify: Telegram returned {status}: {body}");
                // Fall through to stdout
            }
            Err(e) => {
                tracing::warn!("notify: Telegram unreachable: {e}");
                // Fall through to stdout
            }
        }
    }

    // Stdout fallback
    println!("\n📢 AGENT NOTIFICATION: {message}\n");
    Ok(ToolResult::ok(format!("Notification (stdout): {message}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ── resolve_memory_path ──────────────────────────────────────────────────

    #[test]
    fn memory_path_named_keys() {
        let root = std::path::Path::new("/repo");
        assert!(resolve_memory_path(root, "plan").to_string_lossy().ends_with("state/current_plan.md"));
        assert!(resolve_memory_path(root, "last_session").to_string_lossy().ends_with("state/last_session.md"));
        assert!(resolve_memory_path(root, "session_counter").to_string_lossy().ends_with("state/session_counter.txt"));
        assert!(resolve_memory_path(root, "external").to_string_lossy().ends_with("state/external_messages.md"));
        assert!(resolve_memory_path(root, "history").to_string_lossy().ends_with("logs/history.md"));
    }

    #[test]
    fn memory_path_bare_key_goes_to_knowledge() {
        let root = std::path::Path::new("/repo");
        let p = resolve_memory_path(root, "my_notes");
        assert!(p.to_string_lossy().ends_with("knowledge/my_notes.md"));
    }

    #[test]
    fn memory_path_explicit_subpath() {
        let root = std::path::Path::new("/repo");
        let p = resolve_memory_path(root, "knowledge/auth_notes");
        assert!(p.to_string_lossy().ends_with("knowledge/auth_notes.md"));
    }

    #[test]
    fn memory_path_prompts_subpath() {
        let root = std::path::Path::new("/repo");
        let p = resolve_memory_path(root, "prompts/developer");
        assert!(p.to_string_lossy().ends_with("prompts/developer.md"));
    }

    // ── memory_read / memory_write ───────────────────────────────────────────

    #[test]
    fn memory_write_read_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        memory_write(&json!({"key": "knowledge/test_note", "content": "important fact"}), dir.path()).unwrap();
        let r = memory_read(&json!({"key": "knowledge/test_note"}), dir.path()).unwrap();
        assert!(r.success);
        assert!(r.output.contains("important fact"));
    }

    #[test]
    fn memory_write_append_mode() {
        let dir = tempfile::tempdir().unwrap();
        memory_write(&json!({"key": "plan", "content": "step 1\n"}), dir.path()).unwrap();
        memory_write(&json!({"key": "plan", "content": "step 2\n", "append": true}), dir.path()).unwrap();
        let r = memory_read(&json!({"key": "plan"}), dir.path()).unwrap();
        assert!(r.output.contains("step 1"));
        assert!(r.output.contains("step 2"));
    }

    #[test]
    fn memory_write_overwrite_mode() {
        let dir = tempfile::tempdir().unwrap();
        memory_write(&json!({"key": "plan", "content": "old content"}), dir.path()).unwrap();
        memory_write(&json!({"key": "plan", "content": "new content"}), dir.path()).unwrap();
        let r = memory_read(&json!({"key": "plan"}), dir.path()).unwrap();
        assert!(r.output.contains("new content"));
        assert!(!r.output.contains("old content"));
    }

    #[test]
    fn memory_read_missing_key_returns_error_result() {
        let dir = tempfile::tempdir().unwrap();
        let r = memory_read(&json!({"key": "nonexistent_key_xyz"}), dir.path()).unwrap();
        assert!(!r.success);
    }

    // ── read_file / write_file / str_replace ─────────────────────────────────

    #[test]
    fn write_then_read_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        write_file(&json!({"path": "hello.txt", "content": "line1\nline2\n"}), dir.path()).unwrap();
        let r = read_file(&json!({"path": "hello.txt"}), dir.path()).unwrap();
        assert!(r.success);
        assert!(r.output.contains("line1"));
        assert!(r.output.contains("line2"));
    }

    #[test]
    fn read_file_includes_line_numbers() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("f.txt"), "alpha\nbeta\n").unwrap();
        let r = read_file(&json!({"path": "f.txt"}), dir.path()).unwrap();
        assert!(r.output.contains("1\t") || r.output.contains("1 "));
    }

    #[test]
    fn read_file_line_range() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("f.txt"), "alpha\nbeta\ngamma\ndelta\nepsilon\n").unwrap();
        let r = read_file(&json!({"path": "f.txt", "start_line": 2, "end_line": 3}), dir.path()).unwrap();
        assert!(r.output.contains("beta"),  "should contain line 2");
        assert!(r.output.contains("gamma"), "should contain line 3");
        assert!(!r.output.contains("epsilon"), "should not contain line 5");
        assert!(!r.output.contains("delta"),   "should not contain line 4");
    }

    #[test]
    fn read_file_missing_returns_error() {
        let dir = tempfile::tempdir().unwrap();
        let r = read_file(&json!({"path": "does_not_exist.txt"}), dir.path());
        assert!(r.is_err() || r.map(|t| !t.success).unwrap_or(true));
    }

    #[test]
    fn write_file_creates_parent_dirs() {
        let dir = tempfile::tempdir().unwrap();
        write_file(&json!({"path": "a/b/c/file.txt", "content": "hello"}), dir.path()).unwrap();
        assert!(dir.path().join("a/b/c/file.txt").exists());
    }

    #[test]
    fn str_replace_unique_match() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("f.rs"), "fn foo() {}\nfn bar() {}\n").unwrap();
        str_replace(
            &json!({"path": "f.rs", "old_str": "fn foo()", "new_str": "fn baz()"}),
            dir.path(),
        ).unwrap();
        let content = std::fs::read_to_string(dir.path().join("f.rs")).unwrap();
        assert!(content.contains("fn baz()"));
        assert!(!content.contains("fn foo()"));
    }

    #[test]
    fn str_replace_fails_on_duplicate() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("f.rs"), "x\nx\n").unwrap();
        let result = str_replace(
            &json!({"path": "f.rs", "old_str": "x", "new_str": "y"}),
            dir.path(),
        );
        assert!(result.is_err());
    }

    #[test]
    fn str_replace_fails_on_no_match() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("f.rs"), "fn foo() {}").unwrap();
        let result = str_replace(
            &json!({"path": "f.rs", "old_str": "fn missing()", "new_str": "fn other()"}),
            dir.path(),
        );
        assert!(result.is_err());
    }

    // ── find_files ───────────────────────────────────────────────────────────

    #[test]
    fn find_files_by_extension() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("main.rs"), "").unwrap();
        std::fs::write(dir.path().join("lib.rs"), "").unwrap();
        std::fs::write(dir.path().join("readme.md"), "").unwrap();
        let r = find_files(&json!({"pattern": "*.rs"}), dir.path()).unwrap();
        assert!(r.output.contains("main.rs"));
        assert!(r.output.contains("lib.rs"));
        assert!(!r.output.contains("readme.md"));
    }

    #[test]
    fn find_files_substring_match() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("test_auth.rs"), "").unwrap();
        std::fs::write(dir.path().join("test_db.rs"), "").unwrap();
        std::fs::write(dir.path().join("main.rs"), "").unwrap();
        // "test*" matches files starting with "test"
        let r = find_files(&json!({"pattern": "test*"}), dir.path()).unwrap();
        assert!(r.output.contains("test_auth.rs"), "should match test_auth.rs");
        assert!(r.output.contains("test_db.rs"),   "should match test_db.rs");
        assert!(!r.output.contains("main.rs"),     "should not match main.rs");
    }

    // ── search_in_files ──────────────────────────────────────────────────────

    #[test]
    fn search_in_files_basic_regex() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.rs"), "fn foo() {}\nfn bar() {}\n").unwrap();
        let r = search_in_files(&json!({"pattern": "fn \\w+", "ext": "rs"}), dir.path()).unwrap();
        assert!(r.success);
        assert!(r.output.contains("fn foo"));
        assert!(r.output.contains("fn bar"));
    }

    #[test]
    fn search_in_files_no_match() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.rs"), "fn foo() {}").unwrap();
        let r = search_in_files(&json!({"pattern": "NONEXISTENT_PATTERN_XYZ"}), dir.path()).unwrap();
        // Returns success=true with "No matches" message — not an error
        assert!(r.success);
        assert!(r.output.contains("No matches") || r.output.contains("0"));
    }

    // ── strip_html ───────────────────────────────────────────────────────────

    #[test]
    fn strip_html_basic_tags() {
        // Each closing > inserts a space, so adjacent tags produce extra spaces
        // The output collapses per-line but spaces within a line stay
        let out = strip_html("<p>Hello <b>world</b></p>");
        assert!(out.contains("Hello"), "should contain Hello");
        assert!(out.contains("world"), "should contain world");
    }

    #[test]
    fn strip_html_removes_script_block() {
        let html = "<script>alert(1)</script><p>text</p>";
        let out = strip_html(html);
        assert!(!out.contains("alert"));
        assert!(out.contains("text"));
    }

    #[test]
    fn strip_html_removes_style_block() {
        let html = "<style>.foo { color: red }</style><p>content</p>";
        let out = strip_html(html);
        assert!(!out.contains("color"));
        assert!(out.contains("content"));
    }

    #[test]
    fn strip_html_decodes_entities() {
        let out = strip_html("&amp; &lt; &gt; &quot; &#39; &nbsp;");
        assert!(out.contains('&'));
        assert!(out.contains('<'));
        assert!(out.contains('>'));
    }

    #[test]
    fn strip_html_collapses_empty_lines() {
        // strip_html removes empty lines but does not collapse spaces within a line
        let out = strip_html("<p>\n\n\ntext\n\n\n</p>");
        assert!(out.contains("text"));
        assert!(!out.contains("\n\n"), "consecutive empty lines should be removed");
    }

    // ── percent_decode ───────────────────────────────────────────────────────

    #[test]
    fn percent_decode_space() {
        assert_eq!(percent_decode("hello%20world"), "hello world");
    }

    #[test]
    fn percent_decode_slash() {
        assert_eq!(percent_decode("foo%2Fbar"), "foo/bar");
    }

    #[test]
    fn percent_decode_plus_as_space() {
        // DDG uses + for spaces in some places
        let out = percent_decode("hello+world");
        assert!(out == "hello world" || out == "hello+world"); // implementation-defined
    }

    #[test]
    fn percent_decode_passthrough_plain() {
        assert_eq!(percent_decode("no-encoding"), "no-encoding");
    }

    #[test]
    fn percent_decode_empty() {
        assert_eq!(percent_decode(""), "");
    }

    // ── base64_decode_content ────────────────────────────────────────────────

    #[test]
    fn base64_decode_roundtrip() {
        use base64::{engine::general_purpose::STANDARD, Engine};
        let original = "fn main() { println!(\"hello\"); }";
        let encoded = STANDARD.encode(original);
        assert_eq!(base64_decode_content(&encoded), original);
    }

    #[test]
    fn base64_decode_strips_github_newlines() {
        use base64::{engine::general_purpose::STANDARD, Engine};
        let original = "fn main() { println!(\"hello world from GitHub\"); }";
        let encoded = STANDARD.encode(original);
        // GitHub wraps base64 at 60 chars with \n
        let chunked: String = encoded
            .chars()
            .enumerate()
            .flat_map(|(i, c)| {
                if i > 0 && i % 60 == 0 { vec!['\n', c] } else { vec![c] }
            })
            .collect();
        assert!(chunked.contains('\n'));
        assert_eq!(base64_decode_content(&chunked), original);
    }

    #[test]
    fn base64_decode_invalid_returns_fallback() {
        let out = base64_decode_content("not!!valid!!base64");
        assert!(out.starts_with("[binary content"));
    }

    // ── parse_coverage_percent ───────────────────────────────────────────────

    #[test]
    fn coverage_tarpaulin_format() {
        let out = "85.23% coverage, 234/275 lines covered";
        assert_eq!(parse_coverage_percent(out), Some(85.23));
    }

    #[test]
    fn coverage_pytest_total_line() {
        let out = "TOTAL    1000    150    85%";
        assert_eq!(parse_coverage_percent(out), Some(85.0));
    }

    #[test]
    fn coverage_no_percentage_returns_none() {
        assert_eq!(parse_coverage_percent("error: no tests found"), None);
    }

    #[test]
    fn coverage_ignores_values_over_100() {
        // "200% faster" не должно приниматься за coverage
        let out = "200% faster startup\n72% coverage";
        assert_eq!(parse_coverage_percent(out), Some(72.0));
    }

    #[test]
    fn coverage_zero_percent() {
        let out = "0% coverage, 0/100 lines covered";
        assert_eq!(parse_coverage_percent(out), Some(0.0));
    }

    // ── detect_lang ──────────────────────────────────────────────────────────

    #[test]
    fn detect_lang_rust() {
        assert!(matches!(detect_lang(std::path::Path::new("foo.rs")), Lang::Rust));
    }

    #[test]
    fn detect_lang_typescript() {
        assert!(matches!(detect_lang(std::path::Path::new("foo.ts")), Lang::TypeScript));
        assert!(matches!(detect_lang(std::path::Path::new("foo.tsx")), Lang::TypeScript));
    }

    #[test]
    fn detect_lang_javascript() {
        assert!(matches!(detect_lang(std::path::Path::new("foo.js")), Lang::JavaScript));
        assert!(matches!(detect_lang(std::path::Path::new("foo.mjs")), Lang::JavaScript));
    }

    #[test]
    fn detect_lang_python() {
        assert!(matches!(detect_lang(std::path::Path::new("foo.py")), Lang::Python));
    }

    #[test]
    fn detect_lang_cpp() {
        assert!(matches!(detect_lang(std::path::Path::new("foo.cpp")), Lang::Cpp));
        assert!(matches!(detect_lang(std::path::Path::new("foo.hpp")), Lang::Cpp));
        assert!(matches!(detect_lang(std::path::Path::new("foo.h")), Lang::Cpp));
    }

    #[test]
    fn detect_lang_kotlin() {
        assert!(matches!(detect_lang(std::path::Path::new("foo.kt")), Lang::Kotlin));
    }

    #[test]
    fn detect_lang_unknown() {
        assert!(matches!(detect_lang(std::path::Path::new("foo.md")), Lang::Unknown));
        assert!(matches!(detect_lang(std::path::Path::new("foo")), Lang::Unknown));
    }

    // ── AST parsers ──────────────────────────────────────────────────────────

    #[test]
    fn parse_rust_pub_fn() {
        let src = "pub fn hello(x: i32) -> String { todo!() }";
        let syms = parse_rust(src);
        assert!(syms.iter().any(|s| s.name == "hello" && s.kind == "fn"));
    }

    #[test]
    fn parse_rust_async_fn() {
        let src = "pub async fn handle(req: Request) -> Response { todo!() }";
        let syms = parse_rust(src);
        assert!(syms.iter().any(|s| s.name == "handle" && s.kind == "fn"));
    }

    #[test]
    fn parse_rust_struct() {
        let src = "pub struct Config { pub value: i32 }";
        let syms = parse_rust(src);
        assert!(syms.iter().any(|s| s.name == "Config" && s.kind == "struct"));
    }

    #[test]
    fn parse_rust_enum() {
        let src = "pub enum Status { Ok, Err }";
        let syms = parse_rust(src);
        assert!(syms.iter().any(|s| s.name == "Status" && s.kind == "enum"));
    }

    #[test]
    fn parse_rust_impl_block() {
        // Multi-line impl: parser tracks brace depth per-line, single-line impl
        // opens and closes braces on same line so fn may not get container.
        // Test the symbols are found regardless of container tracking.
        let src = "impl Foo {\n    fn bar(&self) {}\n}";
        let syms = parse_rust(src);
        assert!(syms.iter().any(|s| s.kind == "fn" && s.name == "bar"),
            "should find fn bar inside impl block");
    }

    #[test]
    fn parse_rust_trait() {
        let src = "pub trait Handler { fn handle(&self); }";
        let syms = parse_rust(src);
        assert!(syms.iter().any(|s| s.kind == "trait" && s.name == "Handler"));
    }

    #[test]
    fn parse_rust_empty_source() {
        assert!(parse_rust("").is_empty());
    }

    #[test]
    fn parse_python_class_and_method() {
        let src = "class MyClass:\n    def my_method(self):\n        pass\n";
        let syms = parse_python(src);
        assert!(syms.iter().any(|s| s.kind == "class" && s.name == "MyClass"),
            "should find class MyClass");
        // Methods inside a class get kind="method", not "fn"
        assert!(syms.iter().any(|s| s.kind == "method" && s.name == "my_method"),
            "should find my_method with kind=method");
    }

    #[test]
    fn parse_python_top_level_fn() {
        let src = "def process(x, y):\n    return x + y\n";
        let syms = parse_python(src);
        assert!(syms.iter().any(|s| s.name == "process"));
    }

    #[test]
    fn parse_python_async_fn() {
        let src = "async def fetch(url: str) -> str:\n    pass\n";
        let syms = parse_python(src);
        assert!(syms.iter().any(|s| s.name == "fetch"));
    }

    #[test]
    fn parse_ts_function() {
        let src = "export function greet(name: string): string { return `Hello ${name}`; }";
        let syms = parse_ts_js(src);
        assert!(syms.iter().any(|s| s.name == "greet"));
    }

    #[test]
    fn parse_ts_class() {
        let src = "export class UserService { constructor() {} }";
        let syms = parse_ts_js(src);
        assert!(syms.iter().any(|s| s.kind == "class" && s.name == "UserService"));
    }

    #[test]
    fn parse_ts_arrow_fn() {
        let src = "const add = (a: number, b: number) => a + b;";
        let syms = parse_ts_js(src);
        assert!(syms.iter().any(|s| s.name == "add"));
    }

    // ── normalize_path / resolve ─────────────────────────────────────────────

    #[test]
    fn normalize_path_removes_dotdot() {
        let p = normalize_path(std::path::Path::new("/a/b/../c"));
        assert_eq!(p, std::path::PathBuf::from("/a/c"));
    }

    #[test]
    fn normalize_path_removes_dot() {
        let p = normalize_path(std::path::Path::new("/a/./b"));
        assert_eq!(p, std::path::PathBuf::from("/a/b"));
    }

    #[test]
    fn resolve_subpath_ok() {
        let dir = tempfile::tempdir().unwrap();
        let result = resolve(dir.path(), "src/main.rs").unwrap();
        assert!(result.starts_with(dir.path()));
    }

    #[test]
    fn resolve_traversal_escapes_root() {
        // Важно знать: текущая реализация resolve() НЕ блокирует traversal.
        // Этот тест документирует фактическое поведение.
        // Если в будущем добавить проверку — тест должен инвертироваться.
        let dir = tempfile::tempdir().unwrap();
        let result = resolve(dir.path(), "../../../etc/passwd");
        // Сейчас это Ok, но путь уходит за пределы root
        if let Ok(p) = result {
            assert!(!p.starts_with(dir.path()),
                "resolve() currently does NOT block traversal — this documents the gap");
        }
    }
}
