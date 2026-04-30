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

    assert!(err.to_string().contains("llm_malformed_action"));
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

// ─── is_affirmative_response ──────────────────────────────────────────────────
//
// These tests pin the exact set of accepted and rejected inputs.
// is_affirmative_response is called in both run() and run_capture() to decide
// whether the agent continues after a human escalation. A regression here
// (e.g. accidentally accepting "no", or dropping "да") would silently change
// agent behaviour under error conditions.

#[test]
fn affirmative_response_accepts_all_documented_forms() {
    // Every string listed in the function's doc comment must be accepted.
    for input in &[
        "yes", "YES", "Yes",
        "y", "Y",
        "да", "ДА", "Да",
        "д", "Д",
        "continue", "Continue", "CONTINUE",
        "ok", "OK", "Ok",
        "продолжай", "ПРОДОЛЖАЙ",
        "proceed", "Proceed", "PROCEED",
        "go", "GO", "Go",
    ] {
        assert!(
            is_affirmative_response(input),
            "is_affirmative_response({input:?}) must return true"
        );
    }
}

#[test]
fn affirmative_response_accepts_inputs_with_surrounding_whitespace() {
    // The function trims before matching; whitespace variants must be accepted.
    for input in &["  yes  ", "\tyes\n", " да ", " ok\n"] {
        assert!(
            is_affirmative_response(input),
            "is_affirmative_response({input:?}) with whitespace must return true"
        );
    }
}

#[test]
fn affirmative_response_rejects_negative_and_ambiguous_inputs() {
    // None of these should be accepted — any false positive would make the
    // agent continue despite a negative or unclear human response.
    for input in &[
        "no", "NO", "No",
        "n", "N",
        "нет", "нет.",
        "maybe", "perhaps", "later",
        "stop", "exit", "quit",
        "", "   ",
        "yes please",   // extra word — not an exact match
        "yep",
        "sure",
    ] {
        assert!(
            !is_affirmative_response(input),
            "is_affirmative_response({input:?}) must return false"
        );
    }
}

// ─── suppress_human_escalation_for_error ─────────────────────────────────────
//
// suppress_human_escalation_for_error controls whether the 2-consecutive-errors
// threshold triggers ask_human. If a regression makes it return true for a
// non-suppressed error, the agent silently swallows real failures without asking
// the human. If it returns false for a suppressed error, the boss floods the
// human with policy-violation noise.

#[test]
fn suppress_escalation_always_suppresses_stopped_by_user() {
    // "Stopped by user" must be suppressed regardless of role, because the
    // stop was intentional and asking the human is redundant.
    for role in &["boss", "developer", "reviewer", "planner"] {
        assert!(
            suppress_human_escalation_for_error(role, "Stopped by user at step 3"),
            "role={role}: 'Stopped by user' must always suppress escalation"
        );
    }
}

#[test]
fn suppress_escalation_suppresses_boss_policy_violations() {
    // These error messages are produced by Boss policy checks. They are
    // internal invariant violations — asking the human is not useful.
    let boss_policy_errors = [
        "Boss must first delegate",
        "Boss must process the authoritative task source",
        "Boss cannot finish before processing",
        "before asking the human",
    ];
    for msg in &boss_policy_errors {
        assert!(
            suppress_human_escalation_for_error("boss", msg),
            "boss: policy error {msg:?} must suppress escalation"
        );
    }
}

#[test]
fn suppress_escalation_does_not_suppress_boss_policy_violations_for_other_roles() {
    // Boss policy messages must NOT suppress escalation for non-boss roles.
    // Those roles may legitimately encounter similar text in tool output.
    let boss_policy_errors = [
        "Boss must first delegate",
        "Boss must process the authoritative task source",
        "Boss cannot finish before processing",
        "before asking the human",
    ];
    for role in &["developer", "reviewer", "planner"] {
        for msg in &boss_policy_errors {
            assert!(
                !suppress_human_escalation_for_error(role, msg),
                "role={role}: boss policy message must NOT suppress escalation for non-boss role"
            );
        }
    }
}

#[test]
fn suppress_escalation_does_not_suppress_generic_tool_errors() {
    // Ordinary tool failures must not be suppressed — the human should be
    // asked whether to continue after two consecutive failures.
    let generic_errors = [
        "file not found: src/lib.rs",
        "permission denied",
        "network timeout",
        "JSON parse error",
        "llm_malformed_action: model failed twice",
    ];
    for role in &["boss", "developer"] {
        for msg in &generic_errors {
            assert!(
                !suppress_human_escalation_for_error(role, msg),
                "role={role}: generic error {msg:?} must NOT suppress escalation"
            );
        }
    }
}

// ─── await_or_stop ────────────────────────────────────────────────────────────
//
// await_or_stop is the single cancellation point for all blocking agent
// operations: LLM calls, tool dispatch, ask_human. If it regresses (e.g. the
// stop check is removed or the select! arms are swapped), the Stop button
// silently stops working — the agent continues until the Future resolves
// naturally, which may be minutes later (or never, for hung LLM calls).
//
// The tests below are async (tokio::test) because await_or_stop is an async
// function and uses tokio::select! internally.

#[tokio::test]
async fn await_or_stop_returns_error_when_stop_flag_is_already_set() {
    // If AtomicBool is true before await_or_stop is called, the function must
    // return Err("Stopped by user") and not return a successful result.
    //
    // The Future passed here never completes on its own (std::future::pending).
    // Therefore the only way for await_or_stop to return at all is via the
    // stop_future arm inside the select! — which is exactly what we are testing.
    //
    // Note: tokio::select! without `biased` chooses ready arms randomly on the
    // first poll. Using pending() as the main future ensures it is never ready,
    // so stop_future is always the one that resolves.
    use std::sync::atomic::AtomicBool;

    let flag = Arc::new(AtomicBool::new(true));

    let result = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        await_or_stop(Some(flag), std::future::pending::<Result<i32>>()),
    )
    .await
    .expect("await_or_stop must return within 2s when stop flag is already set");

    assert!(result.is_err(), "must return Err when stop flag is set");
    assert!(
        result.unwrap_err().to_string().contains("Stopped by user"),
        "error message must contain 'Stopped by user'"
    );
}

#[tokio::test]
async fn await_or_stop_completes_normally_when_stop_flag_is_not_set() {
    // When AtomicBool stays false the Future must run to completion and its
    // Ok value must be returned unchanged.
    use std::sync::atomic::AtomicBool;

    let flag = Arc::new(AtomicBool::new(false));

    let fut = async { Ok::<i32, anyhow::Error>(99) };

    let result = await_or_stop(Some(flag), fut).await;
    assert!(result.is_ok(), "must return Ok when stop flag is not set");
    assert_eq!(result.unwrap(), 99);
}

#[tokio::test]
async fn await_or_stop_completes_normally_when_no_stop_handle() {
    // None means "no stop handle" — the Future must run to completion without
    // any cancellation check. This is the sub-agent path where no TUI is
    // attached.
    let fut = async { Ok::<&str, anyhow::Error>("done") };

    let result = await_or_stop(None, fut).await;
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), "done");
}

#[tokio::test]
async fn await_or_stop_propagates_future_error_when_stop_flag_is_not_set() {
    // If the Future itself returns Err (e.g. a tool error), that Err must be
    // forwarded unchanged. It must NOT be confused with a "Stopped by user" error.
    use std::sync::atomic::AtomicBool;

    let flag = Arc::new(AtomicBool::new(false));

    let fut = async { anyhow::bail!("tool_error: something went wrong") };

    let result = await_or_stop::<(), _>(Some(flag), fut).await;
    assert!(result.is_err());
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("tool_error"),
        "Future's own error must be forwarded, got: {msg}"
    );
    assert!(
        !msg.contains("Stopped by user"),
        "must not be misidentified as stop, got: {msg}"
    );
}

#[tokio::test]
async fn await_or_stop_stops_when_flag_is_set_during_execution() {
    // The stop flag is set from a background task while await_or_stop is
    // already running. The function must detect it (within the 100ms polling
    // interval) and return Err before the slow Future completes.
    //
    // The Future sleeps for 10 seconds; the stop flag is set after 50ms.
    // We give the test a 2-second budget — well above the 100ms poll interval
    // but far below the 10-second Future duration.
    use std::sync::atomic::{AtomicBool, Ordering};

    let flag = Arc::new(AtomicBool::new(false));
    let flag_setter = Arc::clone(&flag);

    // Set the flag after a short delay so await_or_stop is already inside
    // the select! loop when the cancellation arrives.
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        flag_setter.store(true, Ordering::Relaxed);
    });

    let slow_future = async {
        tokio::time::sleep(std::time::Duration::from_secs(10)).await;
        Ok::<i32, anyhow::Error>(0)
    };

    let result = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        await_or_stop(Some(flag), slow_future),
    )
    .await
    .expect("await_or_stop must return within 2s after stop flag is set");

    assert!(result.is_err(), "must return Err after stop flag is set");
    assert!(
        result.unwrap_err().to_string().contains("Stopped by user"),
        "error must be 'Stopped by user'"
    );
}

// ─── handle_parse_failure: MissingJson and UnterminatedJson variants ──────────
//
// The production match arm is:
//   ParseActionErrorKind::MissingJson
//   | ParseActionErrorKind::UnterminatedJson
//   | ParseActionErrorKind::InvalidJson => { bail after >= 2 }
//
// The existing InvalidJson tests only exercise one arm of the pattern.
// If MissingJson or UnterminatedJson were accidentally removed from the match,
// they would fall through unhandled and the bail!() would never be reached —
// the agent would keep retrying silently instead of terminating.
//
// These tests pin the behaviour of the two remaining variants independently.

#[test]
fn missing_json_first_failure_allows_retry() {
    // First MissingJson: counter goes to 1, outcome is Continue.
    // Guards: variant is inside the MissingJson | UnterminatedJson | InvalidJson arm.
    let mut agent = test_agent(Role::Developer);

    let outcome = handle_parse_failure(
        &mut agent,
        ParseActionError::new(
            ParseActionErrorKind::MissingJson,
            "No JSON block found in model response",
        ),
        "during initial action selection",
    )
    .expect("first MissingJson failure must not bail");

    assert!(matches!(outcome, StepOutcome::Continue));
    assert_eq!(agent.consecutive_parse_failures(), 1);
}

#[test]
fn missing_json_second_failure_bails_with_llm_malformed_action() {
    // Second consecutive MissingJson: counter reaches 2, must bail with
    // "llm_malformed_action". Guards the bail!() path for MissingJson.
    let mut agent = test_agent(Role::Developer);
    agent.set_consecutive_parse_failures(1);

    let err = match handle_parse_failure(
        &mut agent,
        ParseActionError::new(
            ParseActionErrorKind::MissingJson,
            "No JSON block found in model response",
        ),
        "during initial action selection",
    ) {
        Ok(_) => panic!("second MissingJson failure must bail"),
        Err(e) => e,
    };

    assert!(
        err.to_string().contains("llm_malformed_action"),
        "error must contain 'llm_malformed_action', got: {}",
        err
    );
    assert_eq!(agent.consecutive_parse_failures(), 2);
}

#[test]
fn unterminated_json_first_failure_allows_retry() {
    // First UnterminatedJson: counter goes to 1, outcome is Continue.
    // Guards: variant is inside the MissingJson | UnterminatedJson | InvalidJson arm.
    let mut agent = test_agent(Role::Developer);

    let outcome = handle_parse_failure(
        &mut agent,
        ParseActionError::new(
            ParseActionErrorKind::UnterminatedJson,
            "JSON block is not closed — model response was truncated",
        ),
        "during initial action selection",
    )
    .expect("first UnterminatedJson failure must not bail");

    assert!(matches!(outcome, StepOutcome::Continue));
    assert_eq!(agent.consecutive_parse_failures(), 1);
}

#[test]
fn unterminated_json_second_failure_bails_with_llm_malformed_action() {
    // Second consecutive UnterminatedJson: counter reaches 2, must bail with
    // "llm_malformed_action". Guards the bail!() path for UnterminatedJson.
    let mut agent = test_agent(Role::Developer);
    agent.set_consecutive_parse_failures(1);

    let err = match handle_parse_failure(
        &mut agent,
        ParseActionError::new(
            ParseActionErrorKind::UnterminatedJson,
            "JSON block is not closed — model response was truncated",
        ),
        "during initial action selection",
    ) {
        Ok(_) => panic!("second UnterminatedJson failure must bail"),
        Err(e) => e,
    };

    assert!(
        err.to_string().contains("llm_malformed_action"),
        "error must contain 'llm_malformed_action', got: {}",
        err
    );
    assert_eq!(agent.consecutive_parse_failures(), 2);
}

#[test]
fn mixed_malformed_variants_share_the_same_counter_and_bail_threshold() {
    // MissingJson followed by UnterminatedJson must share the same counter —
    // the bail threshold is 2 consecutive malformed actions regardless of
    // which specific variant triggered each failure.
    //
    // This guards against a hypothetical refactor that splits the match arm
    // into separate arms with independent counters, breaking the intended
    // "2 consecutive malformed = stop" invariant.
    let mut agent = test_agent(Role::Developer);

    // First failure: MissingJson — counter → 1, Continue.
    handle_parse_failure(
        &mut agent,
        ParseActionError::new(
            ParseActionErrorKind::MissingJson,
            "No JSON block found",
        ),
        "step 1",
    )
    .expect("first mixed failure (MissingJson) must not bail");
    assert_eq!(agent.consecutive_parse_failures(), 1);

    // Second failure: UnterminatedJson — counter → 2, must bail.
    let err = match handle_parse_failure(
        &mut agent,
        ParseActionError::new(
            ParseActionErrorKind::UnterminatedJson,
            "JSON block is not closed",
        ),
        "step 1 re-route",
    ) {
        Ok(_) => panic!("second mixed failure (UnterminatedJson) must bail"),
        Err(e) => e,
    };

    assert!(
        err.to_string().contains("llm_malformed_action"),
        "mixed variants must share the counter and reach the bail threshold, got: {}",
        err
    );
    assert_eq!(agent.consecutive_parse_failures(), 2);
}
