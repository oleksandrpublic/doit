//! Integration tests for the session lifecycle.
//!
//! These tests exercise session_init → session_finish without an LLM call
//! and verify that the resulting on-disk artifacts are consistent.

use do_it::agent::core::{StopReason, SweAgent};
use do_it::config_struct::{AgentConfig, Role};
use do_it::history::Turn;
use tempfile::TempDir;

fn test_agent(repo: &str) -> SweAgent {
    SweAgent::new(AgentConfig::default(), repo, 5, Role::Developer).unwrap()
}

/// session_init must increment the session counter on disk and session_finish
/// must write the markdown report, the structured trace, and last_session.md.
#[test]
fn session_artifacts_are_written_after_init_and_finish() {
    let repo = TempDir::new().unwrap();
    let root = repo.path();
    let mut agent = test_agent(root.to_str().unwrap());

    agent.session_init();

    // Counter must be written and equal 1 on the first run.
    let counter_path = root.join(".ai").join("state").join("session_counter.txt");
    assert!(counter_path.exists(), "session_counter.txt must exist after init");
    let counter_val = std::fs::read_to_string(&counter_path).unwrap();
    assert_eq!(counter_val.trim(), "1", "counter must be 1 on first session");

    let artifacts = agent
        .session_finish(
            "Verify artifact output",
            "All checks passed.",
            StopReason::Success,
            0,
            std::time::Instant::now(),
            "2024-01-01 00:00:00",
        )
        .expect("top-level session must produce artifacts");

    // Markdown report must exist and contain key fields.
    assert!(artifacts.log_path.exists(), "session report must exist on disk");
    let report = std::fs::read_to_string(&artifacts.log_path).unwrap();
    assert!(report.contains("# Session #1"), "report must reference session number");
    assert!(report.contains("Verify artifact output"), "report must contain task text");
    assert!(report.contains("All checks passed."), "report must contain summary");

    // Structured trace must exist and be valid JSON with expected fields.
    let trace_path = artifacts.trace_path.expect("trace path must be recorded");
    assert!(trace_path.exists(), "trace file must exist on disk");
    let trace_raw = std::fs::read_to_string(&trace_path).unwrap();
    let trace: serde_json::Value = serde_json::from_str(&trace_raw)
        .expect("trace file must be valid JSON");
    assert_eq!(trace["schema_version"], 3);
    assert_eq!(trace["session_nr"], 1);
    assert_eq!(trace["stop_reason"], "success");

    // last_session.md must be written.
    let last_session_path = root.join(".ai").join("state").join("last_session.md");
    assert!(last_session_path.exists(), "last_session.md must exist after finish");
    let last_session = std::fs::read_to_string(&last_session_path).unwrap();
    assert!(!last_session.trim().is_empty(), "last_session.md must not be empty");
    assert!(last_session.contains("Session #1"), "last_session.md must reference session number");
}

/// Running two consecutive sessions must increment the counter to 2 and produce
/// separate report and trace files.
#[test]
fn consecutive_sessions_produce_separate_artifacts() {
    let repo = TempDir::new().unwrap();
    let root = repo.path();

    // First session.
    {
        let mut agent = test_agent(root.to_str().unwrap());
        agent.session_init();
        agent
            .session_finish(
                "First task",
                "First done.",
                StopReason::Success,
                0,
                std::time::Instant::now(),
                "2024-01-01 00:00:00",
            )
            .expect("first session must produce artifacts");
    }

    // Second session.
    {
        let mut agent = test_agent(root.to_str().unwrap());
        agent.session_init();
        let artifacts = agent
            .session_finish(
                "Second task",
                "Second done.",
                StopReason::Success,
                0,
                std::time::Instant::now(),
                "2024-01-01 00:01:00",
            )
            .expect("second session must produce artifacts");

        // Counter must now be 2.
        let counter_val = std::fs::read_to_string(
            root.join(".ai").join("state").join("session_counter.txt"),
        )
        .unwrap();
        assert_eq!(counter_val.trim(), "2", "counter must be 2 after two sessions");

        // The second session's report must reference session #2.
        let report = std::fs::read_to_string(&artifacts.log_path).unwrap();
        assert!(report.contains("# Session #2"), "second report must reference session #2");

        // last_session.md must contain entries from both sessions.
        let last_session = std::fs::read_to_string(
            root.join(".ai").join("state").join("last_session.md"),
        )
        .unwrap();
        assert!(last_session.contains("Session #1"), "last_session must contain session #1 entry");
        assert!(last_session.contains("Session #2"), "last_session must contain session #2 entry");
    }
}

/// A failed (non-success) session must persist task_state.json so the next
/// run can resume, and a successful session must remove it.
#[test]
fn task_state_is_persisted_on_failure_and_cleared_on_success() {
    let repo = TempDir::new().unwrap();
    let root = repo.path();
    let task_state_path = root.join(".ai").join("state").join("task_state.json");

    // Failed session — task_state.json must be written.
    {
        let mut agent = test_agent(root.to_str().unwrap());
        agent.session_init();
        agent.history_mut().push(Turn {
            step: 1,
            thought: "Attempted something".to_string(),
            tool: "read_file".to_string(),
            args: serde_json::json!({ "path": "src/lib.rs" }),
            output: "file not found".to_string(),
            success: false,
        });
        agent
            .session_finish(
                "Fix the bug",
                "Blocked — could not read src/lib.rs.",
                StopReason::Error,
                1,
                std::time::Instant::now(),
                "2024-01-01 00:00:00",
            )
            .expect("failed session must produce artifacts");
    }
    assert!(
        task_state_path.exists(),
        "task_state.json must be written after a failed session"
    );

    // Successful session — task_state.json must be removed.
    {
        let mut agent = test_agent(root.to_str().unwrap());
        agent.session_init();
        agent
            .session_finish(
                "Fix the bug",
                "Done.",
                StopReason::Success,
                0,
                std::time::Instant::now(),
                "2024-01-01 00:01:00",
            )
            .expect("successful session must produce artifacts");
    }
    assert!(
        !task_state_path.exists(),
        "task_state.json must be removed after a successful session"
    );
}
