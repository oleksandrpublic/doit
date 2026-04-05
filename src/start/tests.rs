use super::run_support::{format_config_output, load_run_config, resolve_run_task_input};
use super::status::{
    StatusEmptyState, collect_knowledge_keys, collect_session_artifacts,
    format_session_artifact_summary, format_status_artifact_sections, format_status_body,
    format_status_document_section, format_status_empty_state, format_status_header,
    format_status_knowledge_section, format_status_section_header, format_status_wishlist_section,
    format_trace_path_sensitivity_summary, format_truncated_lines, format_wishlist_summary,
    resolve_status_config_source,
};
use crate::config_loader::LoadedConfig;
use crate::config_struct::{AgentConfig, KNOWLEDGE_DIR, LOGS_DIR, STATE_DIR};
use serial_test::serial;

#[test]
fn collect_session_artifacts_separates_logs_and_traces() {
    let temp = tempfile::TempDir::new().unwrap();
    let logs_dir = temp.path();

    std::fs::write(logs_dir.join("session-001.md"), "report").unwrap();
    std::fs::write(logs_dir.join("session-001.trace.json"), "{}").unwrap();
    std::fs::write(logs_dir.join("session-002.md"), "report").unwrap();
    std::fs::write(logs_dir.join("do_it.log"), "plain log").unwrap();

    let (logs, traces) = collect_session_artifacts(logs_dir);

    assert_eq!(logs.len(), 2);
    assert_eq!(traces.len(), 1);
    assert!(logs.iter().any(|p| p.ends_with("session-001.md")));
    assert!(traces.iter().any(|p| p.ends_with("session-001.trace.json")));
}

#[test]
fn format_session_artifact_summary_reports_latest_trace_and_overflow() {
    let temp = tempfile::TempDir::new().unwrap();
    let logs_dir = temp.path();
    let mut traces = Vec::new();

    for idx in 1..=6 {
        let path = logs_dir.join(format!("session-{idx:03}.trace.json"));
        std::fs::write(&path, format!("trace-{idx}")).unwrap();
        traces.push(path);
    }

    let lines = format_session_artifact_summary("Session traces", "traces", &traces);

    assert!(
        lines
            .iter()
            .any(|line| line.contains("Session traces (6 total):"))
    );
    assert!(
        lines
            .iter()
            .any(|line| line.contains("latest trace:") && line.contains("session-006.trace.json"))
    );
    assert!(
        lines
            .iter()
            .any(|line| line.contains("… and 1 older traces"))
    );
}

#[test]
fn format_session_artifact_summary_reports_none_for_empty_collection() {
    let lines = format_session_artifact_summary("Session logs", "logs", &[]);
    assert_eq!(lines, vec!["  Session logs: none".to_string()]);
}

#[test]
fn format_session_artifact_summary_lists_newest_artifact_first_after_sorting() {
    let temp = tempfile::TempDir::new().unwrap();
    let logs_dir = temp.path();
    let mut logs = vec![
        logs_dir.join("session-003.md"),
        logs_dir.join("session-001.md"),
        logs_dir.join("session-002.md"),
    ];
    for (idx, path) in logs.iter().enumerate() {
        std::fs::write(path, format!("log-{idx}")).unwrap();
    }

    logs.sort();
    let lines = format_session_artifact_summary("Session logs", "logs", &logs);

    assert!(lines[1].contains("session-003.md"));
    assert!(lines[2].contains("session-002.md"));
    assert!(lines[3].contains("session-001.md"));
}

#[test]
fn format_session_artifact_summary_uses_sorted_last_entry_for_latest_trace() {
    let temp = tempfile::TempDir::new().unwrap();
    let logs_dir = temp.path();
    let mut traces = vec![
        logs_dir.join("session-010.trace.json"),
        logs_dir.join("session-002.trace.json"),
        logs_dir.join("session-011.trace.json"),
    ];
    for (idx, path) in traces.iter().enumerate() {
        std::fs::write(path, format!("trace-{idx}")).unwrap();
    }

    traces.sort();
    let lines = format_session_artifact_summary("Session traces", "traces", &traces);

    assert!(lines[1].contains("session-011.trace.json"));
    assert!(
        lines
            .iter()
            .any(|line| line.contains("latest trace:") && line.contains("session-011.trace.json"))
    );
}

#[test]
fn format_status_artifact_sections_lists_logs_then_traces() {
    let temp = tempfile::TempDir::new().unwrap();
    let logs_dir = temp.path();
    let log_path = logs_dir.join("session-001.md");
    let trace_path = logs_dir.join("session-001.trace.json");
    std::fs::write(&log_path, "report").unwrap();
    std::fs::write(&trace_path, "{}").unwrap();

    let lines = format_status_artifact_sections(&[log_path], &[trace_path]);

    assert_eq!(lines[0], "  Session logs (1 total):");
    assert!(lines[1].contains("session-001.md"));
    assert_eq!(lines[2], "  Session traces (1 total):");
    assert!(lines[3].contains("session-001.trace.json"));
    assert!(
        lines
            .iter()
            .any(|line| line.contains("latest trace:") && line.contains("session-001.trace.json"))
    );
}

#[test]
fn format_status_artifact_sections_includes_latest_trace_path_sensitivity_summary() {
    let temp = tempfile::TempDir::new().unwrap();
    let logs_dir = temp.path();
    let trace_path = logs_dir.join("session-001.trace.json");
    std::fs::write(
        &trace_path,
        r#"{
  "path_sensitivity_stats": [
    { "category": "project_config", "calls": 2 },
    { "category": "prompts", "calls": 1 }
  ]
}"#,
    )
    .unwrap();

    let lines = format_status_artifact_sections(&[], &[trace_path]);

    assert!(lines
        .iter()
        .any(|line| line == "    path sensitivity: project_config=2, prompts=1"));
}

#[test]
fn format_trace_path_sensitivity_summary_reports_overflow() {
    let temp = tempfile::TempDir::new().unwrap();
    let trace_path = temp.path().join("session-001.trace.json");
    std::fs::write(
        &trace_path,
        r#"{
  "path_sensitivity_stats": [
    { "category": "project_config", "calls": 3 },
    { "category": "repo_meta", "calls": 2 },
    { "category": "prompts", "calls": 1 },
    { "category": "source", "calls": 1 }
  ]
}"#,
    )
    .unwrap();

    let lines = format_trace_path_sensitivity_summary(&trace_path);

    assert_eq!(
        lines[0],
        "    path sensitivity: project_config=3, repo_meta=2, prompts=1 (+1 more)"
    );
    assert_eq!(lines.len(), 1);
}

#[test]
fn format_trace_path_sensitivity_summary_hides_when_stats_missing() {
    let temp = tempfile::TempDir::new().unwrap();
    let trace_path = temp.path().join("session-001.trace.json");
    std::fs::write(&trace_path, r#"{"tool_stats":[]}"#).unwrap();

    let lines = format_trace_path_sensitivity_summary(&trace_path);

    assert!(lines.is_empty());
}

#[test]
fn format_status_document_section_renders_header_and_truncated_content() {
    let content = "line 1\nline 2\nline 3";

    let lines = format_status_document_section(
        "── Last session ──",
        Some(content),
        2,
        StatusEmptyState::NoLastSession,
        |hidden| format!("… ({hidden} more lines)"),
    );

    assert_eq!(lines[0], "── Last session ──");
    assert_eq!(lines[1], "  line 1");
    assert_eq!(lines[2], "  line 2");
    assert_eq!(lines[3], "  … (1 more lines)");
}

#[test]
fn format_status_document_section_renders_empty_state_when_content_missing() {
    let lines = format_status_document_section(
        "── Current plan ──",
        None,
        30,
        StatusEmptyState::NoPlan,
        |_| "unused".to_string(),
    );

    assert_eq!(lines[0], "── Current plan ──");
    assert_eq!(lines[1], "  (no plan yet)");
}

#[test]
fn format_status_wishlist_section_renders_header_and_empty_state() {
    let lines = format_status_wishlist_section(None);

    assert_eq!(
        lines[0],
        "── Tool wishlist (~/.do_it/tool_wishlist.md) ─────────────────"
    );
    assert_eq!(lines[1], "  (empty)");
}

#[test]
fn format_status_wishlist_section_renders_summary_when_content_exists() {
    let lines = format_status_wishlist_section(Some("## First request\nbody"));

    assert_eq!(
        lines[0],
        "── Tool wishlist (~/.do_it/tool_wishlist.md) ─────────────────"
    );
    assert_eq!(lines[1], "  1 request(s) total");
    assert_eq!(lines[2], "  • First request");
}

#[test]
fn format_status_knowledge_section_hides_when_no_keys_exist() {
    let lines = format_status_knowledge_section(&[]);
    assert!(lines.is_empty());
}

#[test]
fn format_status_knowledge_section_renders_header_and_keys() {
    let lines = format_status_knowledge_section(&["alpha".to_string(), "zeta".to_string()]);

    assert_eq!(
        lines[0],
        "── Knowledge keys (.ai/knowledge/) ───────────────────────────"
    );
    assert_eq!(lines[1], "  • alpha");
    assert_eq!(lines[2], "  • zeta");
}

#[test]
fn format_status_body_preserves_section_order() {
    let temp = tempfile::TempDir::new().unwrap();
    let repo = temp.path();
    let ai = repo.join(".ai");
    std::fs::create_dir_all(ai.join(STATE_DIR)).unwrap();
    std::fs::create_dir_all(ai.join(LOGS_DIR)).unwrap();
    std::fs::create_dir_all(ai.join(KNOWLEDGE_DIR)).unwrap();

    std::fs::write(ai.join(STATE_DIR).join("session_counter.txt"), "7").unwrap();
    std::fs::write(ai.join(LOGS_DIR).join("session-001.md"), "report").unwrap();
    std::fs::write(ai.join(LOGS_DIR).join("session-001.trace.json"), "{}").unwrap();
    std::fs::write(ai.join(STATE_DIR).join("last_session.md"), "done").unwrap();
    std::fs::write(ai.join(STATE_DIR).join("current_plan.md"), "plan").unwrap();
    std::fs::write(ai.join(KNOWLEDGE_DIR).join("alpha.md"), "a").unwrap();

    let lines = format_status_body(repo, &ai);

    let sessions_idx = lines
        .iter()
        .position(|line| line == "  Sessions run: 7")
        .unwrap();
    let logs_idx = lines
        .iter()
        .position(|line| line == "  Session logs (1 total):")
        .unwrap();
    let last_session_idx = lines
        .iter()
        .position(|line| line == "── Last session ──────────────────────────────────────────────")
        .unwrap();
    let plan_idx = lines
        .iter()
        .position(|line| line == "── Current plan ──────────────────────────────────────────────")
        .unwrap();
    let wishlist_idx = lines
        .iter()
        .position(|line| line == "── Tool wishlist (~/.do_it/tool_wishlist.md) ─────────────────")
        .unwrap();
    let knowledge_idx = lines
        .iter()
        .position(|line| line == "── Knowledge keys (.ai/knowledge/) ───────────────────────────")
        .unwrap();

    assert!(sessions_idx < logs_idx);
    assert!(logs_idx < last_session_idx);
    assert!(last_session_idx < plan_idx);
    assert!(plan_idx < wishlist_idx);
    assert!(wishlist_idx < knowledge_idx);
}

#[test]
fn format_truncated_lines_reports_hidden_line_count() {
    let content = (1..=5)
        .map(|n| format!("line {n}"))
        .collect::<Vec<_>>()
        .join("\n");

    let rendered = format_truncated_lines(&content, 3, |hidden| format!("… ({hidden} more lines)"));

    assert_eq!(
        rendered,
        vec![
            "line 1".to_string(),
            "line 2".to_string(),
            "line 3".to_string(),
            "… (2 more lines)".to_string()
        ]
    );
}

#[test]
fn format_truncated_lines_keeps_short_content_unchanged() {
    let content = "line 1\nline 2";
    let rendered = format_truncated_lines(content, 5, |_| "unused".to_string());

    assert_eq!(rendered, vec!["line 1".to_string(), "line 2".to_string()]);
}

#[test]
fn format_wishlist_summary_reports_empty_count_without_overflow_note() {
    let lines = format_wishlist_summary("");
    assert_eq!(lines, vec!["  0 request(s) total".to_string()]);
}

#[test]
fn format_wishlist_summary_lists_latest_three_titles_in_reverse_order() {
    let content = [
        "## First request\nbody",
        "## Second request\nbody",
        "## Third request\nbody",
    ]
    .join("\n");

    let lines = format_wishlist_summary(&content);

    assert_eq!(lines[0], "  3 request(s) total");
    assert_eq!(lines[1], "  • Third request");
    assert_eq!(lines[2], "  • Second request");
    assert_eq!(lines[3], "  • First request");
}

#[test]
fn format_wishlist_summary_adds_overflow_note_for_more_than_three_entries() {
    let content = [
        "## First request\nbody",
        "## Second request\nbody",
        "## Third request\nbody",
        "## Fourth request\nbody",
    ]
    .join("\n");

    let lines = format_wishlist_summary(&content);

    assert_eq!(lines[0], "  4 request(s) total");
    assert_eq!(lines[1], "  • Fourth request");
    assert_eq!(lines[2], "  • Third request");
    assert_eq!(lines[3], "  • Second request");
    assert_eq!(
        lines[4],
        "  … (see ~/.do_it/tool_wishlist.md for full list)"
    );
}

#[test]
fn collect_knowledge_keys_filters_non_markdown_and_sorts_names() {
    let temp = tempfile::TempDir::new().unwrap();
    let dir = temp.path();

    std::fs::write(dir.join("zeta.md"), "z").unwrap();
    std::fs::write(dir.join("alpha.md"), "a").unwrap();
    std::fs::write(dir.join("notes.txt"), "x").unwrap();
    std::fs::create_dir_all(dir.join("nested")).unwrap();

    let keys = collect_knowledge_keys(dir);

    assert_eq!(keys, vec!["alpha".to_string(), "zeta".to_string()]);
}

#[test]
fn collect_knowledge_keys_returns_empty_for_missing_dir() {
    let temp = tempfile::TempDir::new().unwrap();
    let missing = temp.path().join("missing");

    let keys = collect_knowledge_keys(&missing);

    assert!(keys.is_empty());
}

#[test]
fn format_status_empty_state_reports_missing_workspace() {
    let lines = format_status_empty_state(StatusEmptyState::NoWorkspace);
    assert_eq!(
        lines,
        vec![
            "  ⚠  No .ai/ workspace found.".to_string(),
            "     Run `do_it init` to initialise one.".to_string()
        ]
    );
}

#[test]
fn format_status_empty_state_reports_missing_last_session_and_plan() {
    assert_eq!(
        format_status_empty_state(StatusEmptyState::NoLastSession),
        vec!["  (no last_session.md — first run or cleared)".to_string()]
    );
    assert_eq!(
        format_status_empty_state(StatusEmptyState::NoPlan),
        vec!["  (no plan yet)".to_string()]
    );
}

#[test]
fn format_status_empty_state_reports_empty_wishlist() {
    assert_eq!(
        format_status_empty_state(StatusEmptyState::EmptyWishlist),
        vec!["  (empty)".to_string()]
    );
}

#[test]
fn format_status_section_header_shows_always_visible_section_even_when_empty() {
    assert_eq!(
        format_status_section_header("── Current plan ──", false, true),
        Some("── Current plan ──".to_string())
    );
}

#[test]
fn format_status_section_header_hides_optional_section_when_empty() {
    assert_eq!(
        format_status_section_header("── Knowledge keys ──", false, false),
        None
    );
}

#[test]
fn format_status_section_header_shows_optional_section_when_content_exists() {
    assert_eq!(
        format_status_section_header("── Knowledge keys ──", true, false),
        Some("── Knowledge keys ──".to_string())
    );
}

#[test]
fn resolve_status_config_source_reports_repo_config() {
    let temp = tempfile::TempDir::new().unwrap();
    std::fs::write(temp.path().join("config.toml"), "model = \"repo-model\"\n").unwrap();

    let source = resolve_status_config_source(temp.path());

    assert_eq!(
        source,
        format!("repo: {}", temp.path().join("config.toml").display())
    );
}

#[test]
#[serial]
fn resolve_status_config_source_falls_back_to_global_config() {
    let temp = tempfile::TempDir::new().unwrap();
    let home = temp.path().join("home");
    let repo = temp.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::create_dir_all(home.join(".do_it")).unwrap();
    std::fs::write(
        home.join(".do_it").join("config.toml"),
        "model = \"global-model\"\n",
    )
    .unwrap();

    let previous_userprofile = std::env::var("USERPROFILE").ok();
    std::env::set_var("USERPROFILE", &home);

    let source = resolve_status_config_source(&repo);

    match previous_userprofile {
        Some(value) => std::env::set_var("USERPROFILE", value),
        None => std::env::remove_var("USERPROFILE"),
    }

    assert_eq!(
        source,
        format!(
            "global: {}",
            home.join(".do_it").join("config.toml").display()
        )
    );
}

#[test]
fn load_run_config_uses_default_repo_config_when_arg_is_default_name() {
    let temp = tempfile::TempDir::new().unwrap();
    std::fs::write(temp.path().join("config.toml"), "model = \"repo-model\"\n").unwrap();

    let loaded = load_run_config("config.toml", temp.path());

    assert_eq!(loaded.config.model, "repo-model");
    assert_eq!(
        loaded.source,
        format!("repo: {}", temp.path().join("config.toml").display())
    );
}

#[test]
fn resolve_run_task_input_loads_text_file_and_source() {
    let temp = tempfile::TempDir::new().unwrap();
    let task_path = temp.path().join("task.md");
    std::fs::write(&task_path, "Implement the parser.\n").unwrap();

    let (task_image, task_text, task_source) =
        resolve_run_task_input(task_path.to_str().unwrap()).unwrap();

    assert_eq!(task_image, None);
    assert_eq!(task_text, "Implement the parser.");
    assert_eq!(task_source, Some(task_path.display().to_string()));
}

#[test]
fn format_config_output_includes_source_and_pretty_toml() {
    let loaded = LoadedConfig {
        config: AgentConfig {
            model: "test-model".to_string(),
            temperature: 0.2,
            ..AgentConfig::default()
        },
        source: "repo: D:\\test\\32\\config.toml".to_string(),
    };

    let rendered = format_config_output(&loaded).unwrap();

    assert!(rendered.starts_with("# source: repo: D:\\test\\32\\config.toml\n"));
    assert!(rendered.contains("model = \"test-model\""));
    assert!(rendered.contains("temperature = 0.2"));
}

#[test]
fn format_status_header_renders_repo_banner_without_counters() {
    let repo = std::path::Path::new("D:\\test\\32");

    let lines = format_status_header(repo, None);

    assert_eq!(
        lines[0],
        "╭─ do_it status ─────────────────────────────────────────────╮"
    );
    assert_eq!(lines[1], "│ repo: D:\\test\\32");
    assert_eq!(
        lines[2],
        "╰────────────────────────────────────────────────────────────╯"
    );
    assert_eq!(lines[3], "");
    assert_eq!(lines.len(), 4);
}

#[test]
fn format_status_header_renders_session_and_config_counters() {
    let temp = tempfile::TempDir::new().unwrap();
    std::fs::write(temp.path().join("config.toml"), "model = \"repo-model\"\n").unwrap();

    let lines = format_status_header(temp.path(), Some(7));

    assert!(lines.iter().any(|line| line == "  Sessions run: 7"));
    assert!(lines.iter().any(|line| {
        line == &format!(
            "  Config source: repo: {}",
            temp.path().join("config.toml").display()
        )
    }));
}
