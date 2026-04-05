use super::*;
use crate::agent::core::StopReason;
use crate::agent::tools::{ParseActionError, ParseActionErrorKind};
use crate::config_struct::{AgentConfig, Role};
use crate::history::Turn;

fn test_agent(role: Role) -> SweAgent {
    SweAgent::new(
        AgentConfig::default(),
        std::env::current_dir().unwrap().to_str().unwrap(),
        5,
        role,
    )
    .unwrap()
}

#[test]
fn reroute_prompt_carries_first_model_context() {
    let msg = build_reroute_message(
        "original task prompt",
        "thinking-model",
        "coding-model",
        "Inspect the config first",
        "read_file",
        &serde_json::json!({ "path": "src/config.rs" }),
        r#"{"thought":"Inspect the config first","tool":"read_file"}"#,
    );

    assert!(msg.contains("original task prompt"));
    assert!(msg.contains("thinking-model"));
    assert!(msg.contains("coding-model"));
    assert!(msg.contains("Inspect the config first"));
    assert!(msg.contains("read_file"));
    assert!(msg.contains("src/config.rs"));
}

#[test]
fn strategy_notes_warn_before_experimental_loop_hard_stop() {
    let mut agent = test_agent(Role::Boss);
    for step in 1..=2 {
        agent.history_mut().push(Turn {
            step,
            thought: "Need rendered page text".to_string(),
            tool: "browser_get_text".to_string(),
            args: serde_json::json!({ "url": "https://example.com" }),
            output: "[experimental tool]\nERR browser connection is not configured".to_string(),
            success: false,
        });
    }

    let notes = agent
        .build_strategy_notes()
        .expect("strategy note expected");
    assert!(notes.contains("`browser_get_text` is experimental"));
    assert!(notes.contains("Do not call it again unchanged"));
    assert!(notes.contains("`ask_human`") || notes.contains("`notify`"));
}

#[test]
fn strategy_notes_suggest_browser_fallbacks() {
    let mut agent = test_agent(Role::Boss);
    for step in 1..=2 {
        agent.history_mut().push(Turn {
            step,
            thought: "Need text from rendered page".to_string(),
            tool: "browser_get_text".to_string(),
            args: serde_json::json!({ "url": "https://example.com" }),
            output: "[experimental tool]\nERR browser connection is not configured".to_string(),
            success: false,
        });
    }

    let notes = agent
        .build_strategy_notes()
        .expect("strategy note expected");
    assert!(notes.contains("`browser_get_text` is experimental"));
    assert!(notes.contains("`ask_human`") || notes.contains("`notify`"));
}

#[test]
fn strategy_notes_warn_about_repeated_exploration_pressure() {
    let mut agent = test_agent(Role::Developer);
    for (step, tool, args, output) in [
        (
            1,
            "list_dir",
            serde_json::json!({"path":"."}),
            "src\nTODO.md",
        ),
        (
            2,
            "read_file",
            serde_json::json!({"path":"TODO.md"}),
            "File: TODO.md (lines 1-100 of 180)",
        ),
        (
            3,
            "read_file",
            serde_json::json!({"path":"TODO.md"}),
            "File: TODO.md (lines 1-100 of 180)",
        ),
    ] {
        let turn = Turn {
            step,
            thought: "Still exploring".to_string(),
            tool: tool.to_string(),
            args,
            output: output.to_string(),
            success: true,
        };
        agent.task_state_mut().update_from_turn(&turn);
        agent.history_mut().push(turn);
    }

    let notes = agent
        .build_strategy_notes()
        .expect("strategy note expected");
    assert!(notes.contains("Recent steps are dominated by repeated exploration"));
}

#[test]
fn detect_loop_trips_early_for_repeated_exploration_pressure() {
    let mut agent = test_agent(Role::Developer);
    build_test_agent(&mut agent);

    assert!(agent.detect_loop());
}

fn build_test_agent(agent: &mut SweAgent) {
    for (step, tool, args, output) in [
        (
            1,
            "list_dir",
            serde_json::json!({"path":"."}),
            "src\nTODO.md",
        ),
        (
            2,
            "read_file",
            serde_json::json!({"path":"TODO.md"}),
            "File: TODO.md (lines 1-100 of 180)",
        ),
        (
            3,
            "read_file",
            serde_json::json!({"path":"TODO.md"}),
            "File: TODO.md (lines 1-100 of 180)",
        ),
        (
            4,
            "search_in_files",
            serde_json::json!({"pattern":"TODO","dir":"."}),
            "TODO.md:1: do work",
        ),
    ] {
        let turn = Turn {
            step,
            thought: "Still exploring".to_string(),
            tool: tool.to_string(),
            args,
            output: output.to_string(),
            success: true,
        };
        agent.task_state_mut().update_from_turn(&turn);
        agent.history_mut().push(turn);
    }
}

#[test]
fn strategy_notes_require_change_after_exploration_budget_exhausted() {
    let mut agent = test_agent(Role::Developer);
    build_test_agent(&mut agent);
    let notes = agent
        .build_strategy_notes()
        .expect("strategy note expected");
    assert!(notes.contains("The exploration budget is exhausted"));
}

#[test]
fn boss_strategy_notes_warn_about_coordination_loops() {
    let mut agent = test_agent(Role::Boss);
    for (step, tool, args, output) in [
        (
            1,
            "memory_read",
            serde_json::json!({"key":"plan"}),
            "current plan",
        ),
        (
            2,
            "spawn_agent",
            serde_json::json!({"role":"developer","task":"implement x"}),
            "Sub-agent (developer) completed",
        ),
        (
            3,
            "memory_read",
            serde_json::json!({"key":"plan"}),
            "current plan",
        ),
        (
            4,
            "spawn_agent",
            serde_json::json!({"role":"developer","task":"implement x"}),
            "Sub-agent (developer) completed",
        ),
        (
            5,
            "memory_read",
            serde_json::json!({"key":"plan"}),
            "current plan",
        ),
        (
            6,
            "spawn_agent",
            serde_json::json!({"role":"developer","task":"implement x"}),
            "Sub-agent (developer) completed",
        ),
    ] {
        agent.history_mut().push(Turn {
            step,
            thought: "Still coordinating".to_string(),
            tool: tool.to_string(),
            args,
            output: output.to_string(),
            success: true,
        });
    }

    let notes = agent
        .build_strategy_notes()
        .expect("strategy note expected");
    assert!(notes.contains("Boss is coordinating without converging"));
}

#[test]
fn task_state_persists_across_session_roundtrip() {
    let repo = tempfile::TempDir::new().unwrap();
    let repo_path = repo.path().to_str().unwrap().to_string();

    let mut agent = SweAgent::new(AgentConfig::default(), &repo_path, 5, Role::Developer).unwrap();
    agent.session_init();
    agent.task_state_mut().set_goal("Resume after interruption");
    let turn = Turn {
        step: 1,
        thought: "Inspect TODO".to_string(),
        tool: "read_file".to_string(),
        args: serde_json::json!({ "path": "TODO.md" }),
        output: "File: TODO.md (lines 1-100 of 180)".to_string(),
        success: true,
    };
    agent.task_state_mut().update_from_turn(&turn);
    agent.save_task_state();

    let mut restored =
        SweAgent::new(AgentConfig::default(), &repo_path, 5, Role::Developer).unwrap();
    restored.session_init();

    let rendered = restored.task_state().format_for_prompt();
    assert!(rendered.contains("Goal: Resume after interruption"));
    assert!(rendered.contains("read_file"));
    assert!(
        restored
            .history()
            .turns
            .iter()
            .any(|t| t.tool == "load_task_state")
    );
}

#[test]
fn successful_session_finish_clears_persisted_task_state() {
    let repo = tempfile::TempDir::new().unwrap();
    let repo_path = repo.path().to_str().unwrap().to_string();

    let mut agent = SweAgent::new(AgentConfig::default(), &repo_path, 5, Role::Developer).unwrap();
    agent.session_init();
    agent.task_state_mut().set_goal("Complete task");
    agent.save_task_state();
    assert!(agent.task_state_path().exists());

    agent.session_finish(
        "Complete task",
        "done",
        StopReason::Success,
        1,
        std::time::Instant::now(),
        "2024-01-01 00:00:00",
    );
    assert!(!agent.task_state_path().exists());
}

#[test]
fn failed_session_finish_keeps_persisted_task_state() {
    let repo = tempfile::TempDir::new().unwrap();
    let repo_path = repo.path().to_str().unwrap().to_string();

    let mut agent = SweAgent::new(AgentConfig::default(), &repo_path, 5, Role::Developer).unwrap();
    agent.session_init();
    agent.task_state_mut().set_goal("Interrupted task");

    agent.session_finish(
        "Interrupted task",
        "blocked",
        StopReason::Error,
        2,
        std::time::Instant::now(),
        "2024-01-01 00:00:00",
    );
    assert!(agent.task_state_path().exists());
}

#[test]
fn continue_task_uses_restored_goal() {
    let mut agent = test_agent(Role::Developer);
    agent.task_state_mut().set_goal("Fix parser regression");
    assert_eq!(
        agent.resume_effective_task("continue"),
        "Continue the interrupted task: Fix parser regression"
    );
    assert_eq!(
        agent.resume_effective_task("do something else"),
        "do something else"
    );
}

#[test]
fn first_prompt_includes_resume_guidance_when_state_was_restored() {
    let mut agent = test_agent(Role::Developer);
    agent.task_state_mut().set_goal("Finish auth refactor");
    agent.task_state_mut().update_from_turn(&Turn {
        step: 1,
        thought: "Inspected auth module".to_string(),
        tool: "read_file".to_string(),
        args: serde_json::json!({ "path": "src/auth.rs" }),
        output: "module contents".to_string(),
        success: true,
    });
    agent.set_resumed_from_task_state(true);

    let prompt = agent.build_prompt("Continue the interrupted task: Finish auth refactor", 1);
    assert!(prompt.contains("### Resume Guidance"));
    assert!(prompt.contains("Restored goal from persisted task state: Finish auth refactor"));
    assert!(prompt.contains("Last known next best action"));
}

#[test]
fn first_malformed_action_failure_allows_retry() {
    let mut agent = test_agent(Role::Developer);

    let outcome = handle_parse_failure(
        &mut agent,
        ParseActionError::new(
            ParseActionErrorKind::InvalidJson,
            "Invalid action JSON: missing field `tool`",
        ),
        "during initial action selection",
    )
    .expect("first malformed action should not bail");

    assert!(matches!(outcome, StepOutcome::Continue));
    assert_eq!(agent.consecutive_parse_failures(), 1);
}

#[test]
fn repeated_malformed_action_failures_bail_early() {
    let mut agent = test_agent(Role::Developer);
    agent.set_consecutive_parse_failures(1);

    let err = match handle_parse_failure(
        &mut agent,
        ParseActionError::new(
            ParseActionErrorKind::InvalidJson,
            "Invalid action JSON: missing field `tool`",
        ),
        "during initial action selection",
    ) {
        Ok(_) => panic!("second malformed action should bail"),
        Err(err) => err,
    };

    assert!(err.to_string().contains("consecutive malformed actions"));
    assert_eq!(agent.consecutive_parse_failures(), 2);
}

#[test]
fn empty_parse_failures_require_three_consecutive_attempts() {
    let mut agent = test_agent(Role::Developer);

    handle_parse_failure(
        &mut agent,
        ParseActionError::new(
            ParseActionErrorKind::EmptyResponse,
            "LLM response was empty",
        ),
        "during initial action selection",
    )
    .expect("first empty response should continue");
    handle_parse_failure(
        &mut agent,
        ParseActionError::new(
            ParseActionErrorKind::EmptyResponse,
            "LLM response was empty",
        ),
        "during initial action selection",
    )
    .expect("second empty response should continue");

    let err = match handle_parse_failure(
        &mut agent,
        ParseActionError::new(
            ParseActionErrorKind::EmptyResponse,
            "LLM response was empty",
        ),
        "during initial action selection",
    ) {
        Ok(_) => panic!("third empty response should bail"),
        Err(err) => err,
    };

    assert!(
        err.to_string()
            .contains("consecutive empty parse responses")
    );
    assert_eq!(agent.consecutive_parse_failures(), 3);
}

#[test]
fn detect_loop_returns_false_for_clean_history() {
    // An agent with a short, varied, successful history must not trigger
    // loop detection. This guards against regressions that make detect_loop
    // always return true.
    let mut agent = test_agent(Role::Developer);
    for (step, tool, args, output) in [
        (
            1,
            "list_dir",
            serde_json::json!({"path": "."}),
            "src\nCargo.toml",
        ),
        (
            2,
            "read_file",
            serde_json::json!({"path": "Cargo.toml"}),
            "[package]\nname = \"agent\"",
        ),
        (
            3,
            "write_file",
            serde_json::json!({"path": "src/fix.rs", "content": "fn fix() {}"}),
            "File written: src/fix.rs",
        ),
    ] {
        let turn = Turn {
            step,
            thought: "Making progress".to_string(),
            tool: tool.to_string(),
            args,
            output: output.to_string(),
            success: true,
        };
        agent.task_state_mut().update_from_turn(&turn);
        agent.history_mut().push(turn);
    }

    assert!(!agent.detect_loop());
}

#[test]
fn strategy_notes_are_none_for_clean_agent() {
    // A freshly constructed agent with no history must not produce any
    // strategy notes. This guards against regressions that make
    // build_strategy_notes always return Some.
    let agent = test_agent(Role::Developer);
    assert!(agent.build_strategy_notes().is_none());
}

#[test]
fn consecutive_parse_failures_reset_to_zero_after_successful_parse() {
    // Simulates the contract from step(): on a successful parse_action call
    // the loop calls set_consecutive_parse_failures(0). After that reset a
    // single new parse failure must be counted as failure #1, not #N+1.
    // This guards against regressions where the counter is never cleared,
    // which would cause the agent to bail prematurely on the very next error.
    let mut agent = test_agent(Role::Developer);

    // Accumulate one failure so the counter is non-zero.
    handle_parse_failure(
        &mut agent,
        ParseActionError::new(
            ParseActionErrorKind::InvalidJson,
            "Invalid action JSON: missing field `tool`",
        ),
        "during initial action selection",
    )
    .expect("first failure should not bail");
    assert_eq!(agent.consecutive_parse_failures(), 1);

    // Simulate a successful parse: the step loop resets the counter.
    agent.set_consecutive_parse_failures(0);
    assert_eq!(agent.consecutive_parse_failures(), 0);

    // Now a fresh failure must be counted from 1, not from 2.
    // If the counter were not reset this call would bail immediately.
    let outcome = handle_parse_failure(
        &mut agent,
        ParseActionError::new(
            ParseActionErrorKind::InvalidJson,
            "Invalid action JSON: missing field `tool`",
        ),
        "during initial action selection",
    )
    .expect("first failure after reset should not bail");

    assert!(matches!(outcome, StepOutcome::Continue));
    assert_eq!(agent.consecutive_parse_failures(), 1);
}
