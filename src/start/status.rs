use crate::config_struct::{KNOWLEDGE_DIR, LOGS_DIR, STATE_DIR};
use std::path::{Path, PathBuf};

pub fn cmd_status(repo: &str) -> anyhow::Result<()> {
    let root = PathBuf::from(repo)
        .canonicalize()
        .unwrap_or_else(|_| PathBuf::from(repo));
    let ai = root.join(".ai");

    for line in format_status_header(&root, None) {
        println!("{line}");
    }

    if !ai.exists() {
        for line in format_status_empty_state(StatusEmptyState::NoWorkspace) {
            println!("{line}");
        }
        return Ok(());
    }
    for line in format_status_body(&root, &ai) {
        println!("{line}");
    }

    Ok(())
}

pub(crate) fn collect_session_artifacts(logs_dir: &Path) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let mut session_logs = Vec::new();
    let mut session_traces = Vec::new();

    for path in std::fs::read_dir(logs_dir)
        .ok()
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
    {
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !name.starts_with("session-") {
            continue;
        }

        if name.ends_with(".md") {
            session_logs.push(path);
        } else if name.ends_with(".trace.json") {
            session_traces.push(path);
        }
    }

    (session_logs, session_traces)
}

pub(crate) fn format_status_header(repo_root: &Path, session_count: Option<u64>) -> Vec<String> {
    let mut lines = vec![
        "╭─ do_it status ─────────────────────────────────────────────╮".to_string(),
        format!("│ repo: {}", repo_root.display()),
        "╰────────────────────────────────────────────────────────────╯".to_string(),
        String::new(),
    ];

    if let Some(session_count) = session_count {
        lines.push(format!("  Sessions run: {session_count}"));
        lines.push(format!(
            "  Config source: {}",
            resolve_status_config_source(repo_root)
        ));
    }

    lines
}

pub(crate) fn resolve_status_config_source(repo_root: &Path) -> String {
    crate::config_struct::AgentConfig::load_for_repo_with_source(None, Some(repo_root)).source
}

pub(crate) fn format_status_body(repo_root: &Path, ai_dir: &Path) -> Vec<String> {
    let counter_path = ai_dir.join(STATE_DIR).join("session_counter.txt");
    let session_n: u64 = std::fs::read_to_string(&counter_path)
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0);

    let logs_dir = ai_dir.join(LOGS_DIR);
    let (mut session_logs, mut session_traces) = collect_session_artifacts(&logs_dir);
    session_logs.sort();
    session_traces.sort();

    let last_session_path = ai_dir.join(STATE_DIR).join("last_session.md");
    let last_session_content = std::fs::read_to_string(&last_session_path).ok();
    let plan_path = ai_dir.join(STATE_DIR).join("current_plan.md");
    let plan_content = std::fs::read_to_string(&plan_path).ok();
    let wishlist = crate::config_loader::global_tool_wishlist_path()
        .and_then(|p| std::fs::read_to_string(&p).ok());
    let knowledge_dir = ai_dir.join(KNOWLEDGE_DIR);
    let keys = collect_knowledge_keys(&knowledge_dir);

    let mut lines = Vec::new();
    lines.extend(format_status_header(repo_root, Some(session_n)));
    lines.extend(format_status_artifact_sections(
        &session_logs,
        &session_traces,
    ));
    lines.push(String::new());
    lines.extend(format_status_document_section(
        "── Last session ──────────────────────────────────────────────",
        last_session_content.as_deref(),
        40,
        StatusEmptyState::NoLastSession,
        |hidden| format!("  … ({} more lines)", hidden),
    ));
    lines.push(String::new());
    lines.extend(format_status_document_section(
        "── Current plan ──────────────────────────────────────────────",
        plan_content.as_deref(),
        30,
        StatusEmptyState::NoPlan,
        |_| "  … (see .ai/state/current_plan.md for full plan)".to_string(),
    ));
    lines.push(String::new());
    lines.extend(format_status_wishlist_section(wishlist.as_deref()));
    lines.push(String::new());
    lines.extend(format_status_knowledge_section(&keys));
    if !keys.is_empty() {
        lines.push(String::new());
    }
    lines
}

pub(crate) fn format_status_artifact_sections(
    session_logs: &[PathBuf],
    session_traces: &[PathBuf],
) -> Vec<String> {
    let mut lines = format_session_artifact_summary("Session logs", "logs", session_logs);
    lines.extend(format_session_artifact_summary(
        "Session traces",
        "traces",
        session_traces,
    ));
    if let Some(last_trace) = session_traces.last() {
        lines.extend(format_trace_path_sensitivity_summary(last_trace));
    }
    lines
}

pub(crate) fn format_status_document_section(
    heading: &str,
    content: Option<&str>,
    max_lines: usize,
    empty_state: StatusEmptyState,
    overflow_line: impl FnOnce(usize) -> String,
) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(header) = format_status_section_header(heading, true, true) {
        lines.push(header);
    }

    match content {
        Some(content) if !content.trim().is_empty() => {
            lines.extend(
                format_truncated_lines(content, max_lines, overflow_line)
                    .into_iter()
                    .map(|line| format!("  {line}")),
            );
        }
        _ => lines.extend(format_status_empty_state(empty_state)),
    }

    lines
}

pub(crate) fn format_status_wishlist_section(content: Option<&str>) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(header) = format_status_section_header(
        "── Tool wishlist (~/.do_it/tool_wishlist.md) ─────────────────",
        true,
        true,
    ) {
        lines.push(header);
    }

    match content {
        Some(content) if !content.trim().is_empty() => {
            lines.extend(format_wishlist_summary(content))
        }
        _ => lines.extend(format_status_empty_state(StatusEmptyState::EmptyWishlist)),
    }

    lines
}

pub(crate) fn format_status_knowledge_section(keys: &[String]) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(header) = format_status_section_header(
        "── Knowledge keys (.ai/knowledge/) ───────────────────────────",
        !keys.is_empty(),
        false,
    ) {
        lines.push(header);
        lines.extend(keys.iter().map(|key| format!("  • {key}")));
    }
    lines
}

pub(crate) fn format_session_artifact_summary(
    heading: &str,
    artifact_kind_plural: &str,
    paths: &[PathBuf],
) -> Vec<String> {
    if paths.is_empty() {
        return vec![format!("  {heading}: none")];
    }

    let mut lines = vec![format!("  {heading} ({} total):", paths.len())];
    for path in paths.iter().rev().take(5) {
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        lines.push(format!("    • {} ({} bytes)", name, size));
    }

    if artifact_kind_plural == "traces" {
        if let Some(last_trace) = paths.last() {
            lines.push(format!("    latest trace: {}", last_trace.display()));
        }
    }

    if paths.len() > 5 {
        lines.push(format!(
            "    … and {} older {}",
            paths.len() - 5,
            artifact_kind_plural
        ));
    }

    lines
}

pub(crate) fn format_trace_path_sensitivity_summary(trace_path: &Path) -> Vec<String> {
    let Some(stats) = read_trace_path_sensitivity_stats(trace_path) else {
        return Vec::new();
    };
    if stats.is_empty() {
        return Vec::new();
    }

    let summary = stats
        .iter()
        .take(3)
        .map(|(category, calls)| format!("{category}={calls}"))
        .collect::<Vec<_>>()
        .join(", ");

    let mut lines = vec![format!("    path sensitivity: {summary}")];
    if stats.len() > 3 {
        lines[0].push_str(&format!(" (+{} more)", stats.len() - 3));
    }
    lines
}

fn read_trace_path_sensitivity_stats(trace_path: &Path) -> Option<Vec<(String, usize)>> {
    let trace: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(trace_path).ok()?).ok()?;
    let stats = trace
        .get("path_sensitivity_stats")?
        .as_array()?
        .iter()
        .filter_map(|entry| {
            let category = entry.get("category")?.as_str()?.to_string();
            let calls = entry.get("calls")?.as_u64()? as usize;
            Some((category, calls))
        })
        .collect::<Vec<_>>();
    Some(stats)
}

pub(crate) fn format_truncated_lines(
    content: &str,
    max_lines: usize,
    overflow_line: impl FnOnce(usize) -> String,
) -> Vec<String> {
    let lines: Vec<&str> = content.lines().collect();
    let mut rendered = lines
        .iter()
        .take(max_lines)
        .map(|line| line.to_string())
        .collect::<Vec<_>>();

    if lines.len() > max_lines {
        rendered.push(overflow_line(lines.len() - max_lines));
    }

    rendered
}

pub(crate) fn format_wishlist_summary(content: &str) -> Vec<String> {
    let entries: Vec<&str> = content
        .split("\n## ")
        .filter(|s| !s.trim().is_empty())
        .collect();

    let mut lines = vec![format!("  {} request(s) total", entries.len())];
    for entry in entries.iter().rev().take(3) {
        let title = entry
            .lines()
            .next()
            .unwrap_or("")
            .trim_start_matches("## ")
            .trim();
        lines.push(format!("  • {title}"));
    }
    if entries.len() > 3 {
        lines.push("  … (see ~/.do_it/tool_wishlist.md for full list)".to_string());
    }

    lines
}

pub(crate) fn format_status_section_header(
    heading: &str,
    has_content: bool,
    show_when_empty: bool,
) -> Option<String> {
    if has_content || show_when_empty {
        Some(heading.to_string())
    } else {
        None
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StatusEmptyState {
    NoWorkspace,
    NoLastSession,
    NoPlan,
    EmptyWishlist,
}

pub(crate) fn format_status_empty_state(state: StatusEmptyState) -> Vec<String> {
    match state {
        StatusEmptyState::NoWorkspace => vec![
            "  ⚠  No .ai/ workspace found.".to_string(),
            "     Run `do_it init` to initialise one.".to_string(),
        ],
        StatusEmptyState::NoLastSession => {
            vec!["  (no last_session.md — first run or cleared)".to_string()]
        }
        StatusEmptyState::NoPlan => vec!["  (no plan yet)".to_string()],
        StatusEmptyState::EmptyWishlist => vec!["  (empty)".to_string()],
    }
}

pub(crate) fn collect_knowledge_keys(knowledge_dir: &Path) -> Vec<String> {
    let mut keys: Vec<String> = std::fs::read_dir(knowledge_dir)
        .ok()
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) == Some("md") {
                p.file_stem().and_then(|s| s.to_str()).map(String::from)
            } else {
                None
            }
        })
        .collect();
    keys.sort();
    keys
}
