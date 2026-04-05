//! Integration tests for the redaction layer.
//!
//! These tests exercise the full path from sensitive input to on-disk artifact
//! to confirm that no raw secret token survives into written session files.

use do_it::agent::core::SweAgent;
use do_it::agent::core::StopReason;
use do_it::config_struct::{AgentConfig, Role};
use do_it::history::Turn;
use tempfile::TempDir;

fn test_agent(repo: &str) -> SweAgent {
    SweAgent::new(AgentConfig::default(), repo, 5, Role::Developer).unwrap()
}

/// A sensitive token in the task description must not appear verbatim in the
/// written trace JSON file after session_finish.
#[test]
fn redaction_scrubs_sensitive_task_text_from_trace_file() {
    let repo = TempDir::new().unwrap();
    let mut agent = test_agent(repo.path().to_str().unwrap());
    agent.session_init();

    let task = "Deploy service with api_key=ULTRA_SECRET_42 in environment";
    let summary = "Done.";

    let artifacts = agent
        .session_finish(
            task,
            summary,
            StopReason::Success,
            0,
            std::time::Instant::now(),
            "2024-01-01 00:00:00",
        )
        .expect("top-level session must produce artifacts");

    let trace_path = artifacts.trace_path.expect("trace file must be written");
    assert!(trace_path.exists(), "trace file must exist on disk");

    let trace_raw = std::fs::read_to_string(&trace_path).unwrap();
    assert!(
        !trace_raw.contains("ULTRA_SECRET_42"),
        "trace file must not contain raw api_key value; got:\n{trace_raw}"
    );
    assert!(
        trace_raw.contains("[redacted]"),
        "trace file must contain the redaction marker"
    );

    // The markdown report must also be clean.
    let report_raw = std::fs::read_to_string(&artifacts.log_path).unwrap();
    assert!(
        !report_raw.contains("ULTRA_SECRET_42"),
        "session report must not contain raw api_key value"
    );
    assert!(
        report_raw.contains("[redacted]"),
        "session report must contain the redaction marker"
    );
}

/// A sensitive token that appears in a turn's output (e.g. a file read that
/// returned a line with a secret) must not appear in the trace event detail
/// field of the written trace JSON file.
#[test]
fn redaction_scrubs_sensitive_turn_output_from_trace_events() {
    let repo = TempDir::new().unwrap();
    let mut agent = test_agent(repo.path().to_str().unwrap());
    agent.session_init();

    // Simulate a turn whose output contains a raw secret — this is what would
    // happen if read_file returned a config file containing a password line.
    agent.history_mut().push(Turn {
        step: 1,
        thought: "Reading deployment config".to_string(),
        tool: "read_file".to_string(),
        args: serde_json::json!({ "path": "deploy.toml" }),
        output: "host=prod.example.com\npassword=S3CR3T_DEPLOY_99\nport=5432".to_string(),
        success: true,
    });

    let artifacts = agent
        .session_finish(
            "Check deployment config",
            "Done.",
            StopReason::Success,
            1,
            std::time::Instant::now(),
            "2024-01-01 00:00:00",
        )
        .expect("top-level session must produce artifacts");

    let trace_path = artifacts.trace_path.expect("trace file must be written");
    let trace_raw = std::fs::read_to_string(&trace_path).unwrap();

    assert!(
        !trace_raw.contains("S3CR3T_DEPLOY_99"),
        "trace event detail must not contain raw password value; got:\n{trace_raw}"
    );
    assert!(
        trace_raw.contains("[redacted]"),
        "trace must contain the redaction marker in event detail"
    );
}

/// Sensitive token in the final summary must not appear in last_session.md.
#[test]
fn redaction_scrubs_sensitive_summary_from_last_session() {
    let repo = TempDir::new().unwrap();
    let mut agent = test_agent(repo.path().to_str().unwrap());
    agent.session_init();

    let summary = "Completed. Auth token was ghp_LEAKED_TOKEN_XYZ used during run.";

    agent
        .session_finish(
            "Run integration",
            summary,
            StopReason::Success,
            0,
            std::time::Instant::now(),
            "2024-01-01 00:00:00",
        )
        .expect("top-level session must produce artifacts");

    let last_session_path = repo
        .path()
        .join(".ai")
        .join("state")
        .join("last_session.md");
    assert!(last_session_path.exists(), "last_session.md must be written");

    let last_session_raw = std::fs::read_to_string(&last_session_path).unwrap();
    assert!(
        !last_session_raw.contains("ghp_LEAKED_TOKEN_XYZ"),
        "last_session.md must not contain raw GitHub token; got:\n{last_session_raw}"
    );
    assert!(
        last_session_raw.contains("[redacted]"),
        "last_session.md must contain the redaction marker"
    );
}
