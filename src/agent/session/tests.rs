#[cfg(test)]
mod tests {
    use crate::agent::core::{StopReason, SweAgent};
    use crate::agent::session::persistence::TaskStatePersistenceAction;
    use crate::agent::session::trace::SessionTracePathSensitivityStat;
    use crate::config_struct::{AgentConfig, Role};
    use crate::history::Turn;

    fn test_agent(repo: &str) -> SweAgent {
        SweAgent::new(AgentConfig::default(), repo, 5, Role::Developer).unwrap()
    }

    #[test]
    fn persistence_action_clears_state_after_success() {
        let repo = tempfile::TempDir::new().unwrap();
        let agent = test_agent(repo.path().to_str().unwrap());

        assert_eq!(
            agent.task_state_persistence_action(StopReason::Success),
            TaskStatePersistenceAction::Clear
        );
    }

    #[test]
    fn persistence_action_saves_state_after_failure() {
        let repo = tempfile::TempDir::new().unwrap();
        let agent = test_agent(repo.path().to_str().unwrap());

        assert_eq!(
            agent.task_state_persistence_action(StopReason::Error),
            TaskStatePersistenceAction::Save
        );
        assert_eq!(
            agent.task_state_persistence_action(StopReason::NoProgress),
            TaskStatePersistenceAction::Save
        );
        assert_eq!(
            agent.task_state_persistence_action(StopReason::MaxSteps),
            TaskStatePersistenceAction::Save
        );
    }

    #[test]
    fn append_decision_writes_structured_entry() {
        let repo = tempfile::TempDir::new().unwrap();
        let mut agent = test_agent(repo.path().to_str().unwrap());
        agent.session_init();

        agent.append_decision(3, "read_file", "Using read_file instead of outline because I need the full body");

        let decisions_path = repo.path()
            .join(".ai").join("state").join("session_decisions.md");
        assert!(decisions_path.exists(), "session_decisions.md must be created");

        let content = std::fs::read_to_string(&decisions_path).unwrap();
        assert!(content.contains("Step 3"), "entry must include step number");
        assert!(content.contains("[developer]"), "entry must include role");
        assert!(content.contains("Tool: read_file"), "entry must include tool");
        assert!(
            content.contains("Using read_file instead of outline"),
            "entry must include decision text"
        );
    }

    #[test]
    fn append_decision_accumulates_multiple_entries() {
        let repo = tempfile::TempDir::new().unwrap();
        let mut agent = test_agent(repo.path().to_str().unwrap());
        agent.session_init();

        agent.append_decision(1, "outline", "outline first to get symbol locations");
        agent.append_decision(2, "str_replace", "str_replace preferred over write_file for targeted edit");

        let content = std::fs::read_to_string(
            repo.path().join(".ai").join("state").join("session_decisions.md"),
        ).unwrap();
        assert!(content.contains("Step 1"), "first entry must be present");
        assert!(content.contains("Step 2"), "second entry must be present");
        assert!(content.contains("outline first"), "first decision text must be present");
        assert!(content.contains("str_replace preferred"), "second decision text must be present");
    }

    #[test]
    fn append_decision_ignores_empty_decision() {
        let repo = tempfile::TempDir::new().unwrap();
        let mut agent = test_agent(repo.path().to_str().unwrap());
        agent.session_init();

        agent.append_decision(1, "read_file", "   ");

        let decisions_path = repo.path()
            .join(".ai").join("state").join("session_decisions.md");
        assert!(
            !decisions_path.exists() || std::fs::read_to_string(&decisions_path).unwrap().trim().is_empty(),
            "empty decision must not create a non-empty file"
        );
    }

    #[test]
    fn resume_guidance_mentions_recent_path_sensitive_writes() {
        let repo = tempfile::TempDir::new().unwrap();
        let mut agent = test_agent(repo.path().to_str().unwrap());
        agent.set_resumed_from_task_state(true);
        agent.history_mut().push(Turn {
            step: 0,
            thought: "Loading previous session context".to_string(),
            tool: "load_session".to_string(),
            args: serde_json::json!({}),
            output: "## Session 7\n\n**Safety:** project_config=1, prompts=1".to_string(),
            success: true,
        });

        let guidance = agent
            .resume_guidance()
            .expect("resume guidance should be present");

        assert!(guidance.contains(
            "Recent path-sensitive writes from the previous session: project_config=1, prompts=1."
        ));
        assert!(guidance.contains(
            "Verify those areas before broad follow-up changes."
        ));
    }

    #[test]
    fn resume_guidance_omits_safety_line_when_last_session_has_no_safety_summary() {
        let repo = tempfile::TempDir::new().unwrap();
        let mut agent = test_agent(repo.path().to_str().unwrap());
        agent.set_resumed_from_task_state(true);
        agent.history_mut().push(Turn {
            step: 0,
            thought: "Loading previous session context".to_string(),
            tool: "load_session".to_string(),
            args: serde_json::json!({}),
            output: "## Session 7\n\nSummary only".to_string(),
            success: true,
        });

        assert!(agent.resume_guidance().is_none());
    }

    #[test]
    fn restore_task_state_from_disk_returns_false_when_missing() {
        let repo = tempfile::TempDir::new().unwrap();
        let mut agent = test_agent(repo.path().to_str().unwrap());

        assert!(!agent.restore_task_state_from_disk());
        assert!(!agent.resumed_from_task_state());
    }

    #[test]
    fn restore_task_state_from_disk_returns_false_for_corrupt_json() {
        let repo = tempfile::TempDir::new().unwrap();
        let mut agent = test_agent(repo.path().to_str().unwrap());
        let path = agent.task_state_path();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "{ this is not valid json").unwrap();

        assert!(!agent.restore_task_state_from_disk());
        assert!(!agent.resumed_from_task_state());
        assert!(
            agent
                .history()
                .turns
                .iter()
                .all(|t| t.tool != "load_task_state")
        );
    }

    #[test]
    fn restore_task_state_from_disk_returns_false_for_semantically_empty_state() {
        let repo = tempfile::TempDir::new().unwrap();
        let mut agent = test_agent(repo.path().to_str().unwrap());
        let path = agent.task_state_path();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "{}").unwrap();

        assert!(!agent.restore_task_state_from_disk());
        assert!(!agent.resumed_from_task_state());
        assert!(
            agent
                .history()
                .turns
                .iter()
                .all(|t| t.tool != "load_task_state")
        );
    }

    #[test]
    fn restore_task_state_from_disk_accepts_partial_but_actionable_state() {
        let repo = tempfile::TempDir::new().unwrap();
        let mut agent = test_agent(repo.path().to_str().unwrap());
        let path = agent.task_state_path();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            r#"{
  "goal": "",
  "attempted_actions": [],
  "artifacts_found": [],
  "blocked_on": [],
  "repeated_signatures": [],
  "recent_signatures": [],
  "recent_progress_markers": [],
  "next_best_action": "Run focused verification before broader edits."
}"#,
        )
        .unwrap();

        assert!(agent.restore_task_state_from_disk());
        assert!(agent.resumed_from_task_state());
        assert_eq!(
            agent.task_state().next_best_action_hint(),
            Some("Run focused verification before broader edits.")
        );
    }

    #[test]
    fn session_finish_writes_structured_trace_file() {
        let repo = tempfile::TempDir::new().unwrap();
        let repo_path = repo.path().to_str().unwrap();
        let mut agent = test_agent(repo_path);
        agent.session_init();
        agent.history_mut().push(Turn {
            step: 1,
            thought: "Inspect config".to_string(),
            tool: "read_file".to_string(),
            args: serde_json::json!({ "path": "config.toml" }),
            output: "llm_backend = \"ollama\"".to_string(),
            success: true,
        });

        let artifacts = agent
            .session_finish(
                "Inspect config precedence",
                "done",
                StopReason::Success,
                1,
                std::time::Instant::now(),
                "2024-01-01 00:00:00",
            )
            .expect("top-level session should produce artifacts");

        let trace_path = artifacts.trace_path.expect("trace path should be recorded");
        assert!(trace_path.exists());

        let trace = std::fs::read_to_string(trace_path).unwrap();
        assert!(trace.contains("\"schema_version\": 4"));
        assert!(trace.contains("\"max_steps\": 5"));
        assert!(trace.contains("\"config_source\": \"built-in defaults\""));
        assert!(trace.contains("\"path_sensitivity_stats\""));
        assert!(trace.contains("\"event\": \"session_started\""));
        assert!(trace.contains("\"event\": \"turn\""));
        assert!(trace.contains("\"event\": \"session_finished\""));
        assert!(trace.contains("\"stop_reason\": \"success\""));
    }

    #[test]
    fn session_finish_report_includes_path_sensitivity_section_when_present() {
        let repo = tempfile::TempDir::new().unwrap();
        let repo_path = repo.path().to_str().unwrap();
        let mut agent = test_agent(repo_path);
        agent.session_init();
        agent.history_mut().push(Turn {
            step: 1,
            thought: "Edit config".to_string(),
            tool: "write_file".to_string(),
            args: serde_json::json!({ "path": "config.toml" }),
            output: "Written [sensitivity: project_config]".to_string(),
            success: true,
        });
        agent.history_mut().push(Turn {
            step: 2,
            thought: "Edit prompt".to_string(),
            tool: "str_replace".to_string(),
            args: serde_json::json!({ "path": ".ai/prompts/boss.md" }),
            output: "Replaced [sensitivity: prompts]".to_string(),
            success: true,
        });

        let artifacts = agent
            .session_finish(
                "Tune safety diagnostics",
                "done",
                StopReason::Success,
                2,
                std::time::Instant::now(),
                "2024-01-01 00:00:00",
            )
            .expect("top-level session should produce artifacts");

        let report = std::fs::read_to_string(artifacts.log_path).unwrap();
        assert!(report.contains("## Path sensitivity"));
        assert!(report.contains("  - project_config: 1 call(s)"));
        assert!(report.contains("  - prompts: 1 call(s)"));
    }

    #[test]
    fn last_session_entry_includes_safety_summary_when_present() {
        let repo = tempfile::TempDir::new().unwrap();
        let repo_path = repo.path().to_str().unwrap();
        let mut agent = test_agent(repo_path);
        agent.session_init();
        agent.history_mut().push(Turn {
            step: 1,
            thought: "Edit config".to_string(),
            tool: "write_file".to_string(),
            args: serde_json::json!({ "path": "config.toml" }),
            output: "Written [sensitivity: project_config]".to_string(),
            success: true,
        });

        agent.session_finish(
            "Tune safety diagnostics",
            "done",
            StopReason::Success,
            1,
            std::time::Instant::now(),
            "2024-01-01 00:00:00",
        );

        let last_session = std::fs::read_to_string(
            repo.path().join(".ai").join("state").join("last_session.md"),
        )
        .unwrap();
        assert!(last_session.contains("**Safety:** project_config=1"));
    }

    #[test]
    fn trace_preview_normalizes_newlines_and_truncates() {
        let preview = SweAgent::trace_preview("line 1\nline 2\nline 3", 10);
        assert_eq!(preview, "line 1\\nli...");

        let empty = SweAgent::trace_preview(" \n\t ", 10);
        assert_eq!(empty, "(empty)");
    }

    #[test]
    fn trace_preview_redacts_sensitive_lines() {
        let input = "normal line\napi_key=supersecret\nother line";
        let preview = SweAgent::trace_preview(input, 200);
        assert!(!preview.contains("supersecret"));
        assert!(preview.contains("[redacted]"));
        assert!(preview.contains("normal line"));
        assert!(preview.contains("other line"));
    }

    #[test]
    fn trace_events_include_start_turn_and_finish_metadata() {
        let repo = tempfile::TempDir::new().unwrap();
        let agent = test_agent(repo.path().to_str().unwrap());
        let turn = Turn {
            step: 2,
            thought: "Inspect config".to_string(),
            tool: "read_file".to_string(),
            args: serde_json::json!({ "path": "config.toml" }),
            output: "llm_backend = \"ollama\"\nmodel = \"qwen\" [sensitivity: project_config]"
                .to_string(),
            success: true,
        };
        let turns = vec![&turn];

        let events = agent.trace_events("Investigate config flow", StopReason::NoProgress, &turns);

        assert_eq!(events.len(), 3);
        assert_eq!(events[0].event, "session_started");
        assert_eq!(events[0].detail, "Investigate config flow");
        assert_eq!(events[1].event, "turn");
        assert_eq!(events[1].step, Some(2));
        assert_eq!(events[1].tool, Some("read_file"));
        assert_eq!(events[1].success, Some(true));
        assert!(events[1].detail.contains("thought=Inspect config"));
        assert!(events[1].detail.contains("sensitivity=project_config"));
        assert!(
            events[1]
                .detail
                .contains("output=llm_backend = \"ollama\"\\nmodel = \"qwen\"")
        );
        assert!(
            events[1].args_preview.contains("config.toml"),
            "turn args_preview must contain the path argument: {}", events[1].args_preview
        );
        assert!(events[0].args_preview.is_empty(), "session_started args_preview must be empty");
        assert!(events[2].args_preview.is_empty(), "session_finished args_preview must be empty");
        assert_eq!(events[2].event, "session_finished");
        assert_eq!(events[2].success, Some(false));
        assert!(events[2].detail.contains("stop_reason=no_progress"));
        assert!(
            events[2]
                .detail
                .contains("final_output=llm_backend = \"ollama\"\\nmodel = \"qwen\"")
        );
    }

    #[test]
    fn trace_tool_stats_aggregate_calls_and_errors() {
        let turn1 = Turn {
            step: 1,
            thought: "Read config".to_string(),
            tool: "read_file".to_string(),
            args: serde_json::json!({ "path": "config.toml" }),
            output: "ok".to_string(),
            success: true,
        };
        let turn2 = Turn {
            step: 2,
            thought: "Read another file".to_string(),
            tool: "read_file".to_string(),
            args: serde_json::json!({ "path": "Cargo.toml" }),
            output: "ok".to_string(),
            success: true,
        };
        let turn3 = Turn {
            step: 3,
            thought: "Run tests".to_string(),
            tool: "run_command".to_string(),
            args: serde_json::json!({ "program": "cargo test" }),
            output: "ERR failed".to_string(),
            success: false,
        };
        let turns = vec![&turn1, &turn2, &turn3];

        let stats = SweAgent::trace_tool_stats(&turns);

        assert_eq!(stats.len(), 2);
        assert_eq!(stats[0].tool, "read_file");
        assert_eq!(stats[0].calls, 2);
        assert_eq!(stats[0].err_calls, 0);
        assert_eq!(stats[1].tool, "run_command");
        assert_eq!(stats[1].calls, 1);
        assert_eq!(stats[1].err_calls, 1);
    }

    #[test]
    fn trace_path_sensitivity_stats_aggregate_tagged_turns() {
        let turn1 = Turn {
            step: 1,
            thought: "Write config".to_string(),
            tool: "write_file".to_string(),
            args: serde_json::json!({ "path": "config.toml" }),
            output: "Written [sensitivity: project_config]".to_string(),
            success: true,
        };
        let turn2 = Turn {
            step: 2,
            thought: "Edit prompt".to_string(),
            tool: "str_replace".to_string(),
            args: serde_json::json!({ "path": ".ai/prompts/boss.md" }),
            output: "Replaced [sensitivity: prompts]".to_string(),
            success: true,
        };
        let turn3 = Turn {
            step: 3,
            thought: "Write config again".to_string(),
            tool: "write_file".to_string(),
            args: serde_json::json!({ "path": "config.toml" }),
            output: "Written [sensitivity: project_config]".to_string(),
            success: false,
        };
        let turns = vec![&turn1, &turn2, &turn3];

        let stats = SweAgent::trace_path_sensitivity_stats(&turns);

        assert_eq!(stats.len(), 2);
        assert_eq!(stats[0].category, "project_config");
        assert_eq!(stats[0].calls, 2);
        assert_eq!(stats[1].category, "prompts");
        assert_eq!(stats[1].calls, 1);
    }

    #[test]
    fn render_path_sensitivity_report_section_hides_when_empty() {
        let rendered = SweAgent::render_path_sensitivity_report_section(&[]);
        assert!(rendered.is_empty());
    }

    #[test]
    fn render_path_sensitivity_report_section_lists_categories() {
        let rendered = SweAgent::render_path_sensitivity_report_section(&[
            SessionTracePathSensitivityStat {
                category: "project_config",
                calls: 2,
            },
            SessionTracePathSensitivityStat {
                category: "prompts",
                calls: 1,
            },
        ]);

        assert!(rendered.contains("## Path sensitivity"));
        assert!(rendered.contains("  - project_config: 2 call(s)"));
        assert!(rendered.contains("  - prompts: 1 call(s)"));
    }

    #[test]
    fn render_path_sensitivity_summary_line_hides_when_empty() {
        let rendered = SweAgent::render_path_sensitivity_summary_line(&[]);
        assert_eq!(rendered, None);
    }

    #[test]
    fn render_path_sensitivity_summary_line_compacts_categories() {
        let rendered = SweAgent::render_path_sensitivity_summary_line(&[
            SessionTracePathSensitivityStat {
                category: "project_config",
                calls: 2,
            },
            SessionTracePathSensitivityStat {
                category: "prompts",
                calls: 1,
            },
        ]);

        assert_eq!(rendered, Some("project_config=2, prompts=1".to_string()));
    }

    #[test]
    fn render_path_sensitivity_summary_line_reports_overflow() {
        let rendered = SweAgent::render_path_sensitivity_summary_line(&[
            SessionTracePathSensitivityStat {
                category: "project_config",
                calls: 3,
            },
            SessionTracePathSensitivityStat {
                category: "repo_meta",
                calls: 2,
            },
            SessionTracePathSensitivityStat {
                category: "prompts",
                calls: 1,
            },
            SessionTracePathSensitivityStat {
                category: "source",
                calls: 1,
            },
        ]);

        assert_eq!(
            rendered,
            Some("project_config=3, repo_meta=2, prompts=1 (+1 more)".to_string())
        );
    }

    #[test]
    fn render_final_summary_lines_include_safety_summary_when_present() {
        let repo = tempfile::TempDir::new().unwrap();
        let repo_path = repo.path().to_str().unwrap();
        let mut agent = test_agent(repo_path);
        agent.session_init();
        agent.history_mut().push(Turn {
            step: 1,
            thought: "Edit config".to_string(),
            tool: "write_file".to_string(),
            args: serde_json::json!({ "path": "config.toml" }),
            output: "Written [sensitivity: project_config]".to_string(),
            success: true,
        });
        let artifacts = agent
            .session_finish(
                "Tune safety diagnostics",
                "done",
                StopReason::Success,
                1,
                std::time::Instant::now(),
                "2024-01-01 00:00:00",
            )
            .expect("top-level session should produce artifacts");

        let lines =
            agent.render_final_summary_lines(StopReason::Success, "done", 1, Some(&artifacts));

        assert!(lines
            .iter()
            .any(|line| line == "Safety : project_config=1"));
    }

    #[test]
    fn render_final_summary_lines_include_safety_overflow_summary_when_needed() {
        let repo = tempfile::TempDir::new().unwrap();
        let repo_path = repo.path().to_str().unwrap();
        let mut agent = test_agent(repo_path);
        agent.session_init();
        agent.history_mut().push(Turn {
            step: 1,
            thought: "Edit config".to_string(),
            tool: "write_file".to_string(),
            args: serde_json::json!({ "path": "config.toml" }),
            output: "Written [sensitivity: project_config]".to_string(),
            success: true,
        });
        agent.history_mut().push(Turn {
            step: 2,
            thought: "Edit git config".to_string(),
            tool: "write_file".to_string(),
            args: serde_json::json!({ "path": ".git/config" }),
            output: "Written [sensitivity: repo_meta]".to_string(),
            success: true,
        });
        agent.history_mut().push(Turn {
            step: 3,
            thought: "Edit prompt".to_string(),
            tool: "str_replace".to_string(),
            args: serde_json::json!({ "path": ".ai/prompts/boss.md" }),
            output: "Replaced [sensitivity: prompts]".to_string(),
            success: true,
        });
        agent.history_mut().push(Turn {
            step: 4,
            thought: "Edit source".to_string(),
            tool: "write_file".to_string(),
            args: serde_json::json!({ "path": "src/lib.rs" }),
            output: "Written [sensitivity: source]".to_string(),
            success: true,
        });
        let artifacts = agent
            .session_finish(
                "Tune safety diagnostics",
                "done",
                StopReason::Success,
                4,
                std::time::Instant::now(),
                "2024-01-01 00:00:00",
            )
            .expect("top-level session should produce artifacts");

        let lines =
            agent.render_final_summary_lines(StopReason::Success, "done", 4, Some(&artifacts));

        assert!(lines.iter().any(|line| {
            line == "Safety : project_config=1, prompts=1, repo_meta=1 (+1 more)"
        }));
    }

    #[test]
    fn last_session_entry_omits_safety_summary_when_absent() {
        let repo = tempfile::TempDir::new().unwrap();
        let repo_path = repo.path().to_str().unwrap();
        let mut agent = test_agent(repo_path);
        agent.session_init();
        agent.history_mut().push(Turn {
            step: 1,
            thought: "Read config".to_string(),
            tool: "read_file".to_string(),
            args: serde_json::json!({ "path": "config.toml" }),
            output: "ok".to_string(),
            success: true,
        });

        agent.session_finish(
            "Inspect config",
            "done",
            StopReason::Success,
            1,
            std::time::Instant::now(),
            "2024-01-01 00:00:00",
        );

        let last_session = std::fs::read_to_string(
            repo.path().join(".ai").join("state").join("last_session.md"),
        )
        .unwrap();
        assert!(!last_session.contains("**Safety:**"));
    }

    #[test]
    fn structured_trace_json_contains_expected_metadata_counts() {
        let repo = tempfile::TempDir::new().unwrap();
        let repo_path = repo.path().to_str().unwrap();
        let mut agent = test_agent(repo_path);
        agent.session_init();
        agent.history_mut().push(Turn {
            step: 1,
            thought: "Inspect config".to_string(),
            tool: "read_file".to_string(),
            args: serde_json::json!({ "path": "config.toml" }),
            output: "ok [sensitivity: project_config]".to_string(),
            success: true,
        });
        agent.history_mut().push(Turn {
            step: 2,
            thought: "Run test".to_string(),
            tool: "run_command".to_string(),
            args: serde_json::json!({ "program": "cargo test" }),
            output: "ERR failed".to_string(),
            success: false,
        });

        let artifacts = agent
            .session_finish(
                "Inspect config precedence",
                "blocked",
                StopReason::Error,
                2,
                std::time::Instant::now(),
                "2024-01-01 00:00:00",
            )
            .expect("top-level session should produce artifacts");

        let trace_path = artifacts.trace_path.expect("trace path should be recorded");
        let trace: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(trace_path).unwrap()).unwrap();

        assert_eq!(trace["schema_version"], 4);
        assert_eq!(trace["role"], "developer");
        assert_eq!(trace["config_source"], "built-in defaults");
        assert_eq!(trace["stop_reason"], "error");
        assert_eq!(trace["started_at"], "2024-01-01 00:00:00");
        assert_eq!(trace["max_steps"], 5);
        assert_eq!(trace["steps_used"], 2);
        assert_eq!(trace["resumed_from_task_state"], false);
        assert_eq!(trace["summary_preview"], "blocked");
        assert_eq!(trace["total_calls"], 2);
        assert_eq!(trace["ok_calls"], 1);
        assert_eq!(trace["err_calls"], 1);
        assert_eq!(trace["tool_stats"].as_array().unwrap().len(), 2);
        assert_eq!(trace["tool_stats"][0]["tool"], "read_file");
        assert_eq!(trace["tool_stats"][0]["calls"], 1);
        assert_eq!(trace["tool_stats"][0]["err_calls"], 0);
        assert_eq!(trace["path_sensitivity_stats"].as_array().unwrap().len(), 1);
        assert_eq!(
            trace["path_sensitivity_stats"][0]["category"],
            "project_config"
        );
        assert_eq!(trace["path_sensitivity_stats"][0]["calls"], 1);
        assert_eq!(trace["events"].as_array().unwrap().len(), 4);
        assert_eq!(
            trace["events"][1]["detail"],
            "thought=Inspect config sensitivity=project_config output=ok [sensitivity: project_config]"
        );
        assert!(
            trace["events"][1]["args_preview"].as_str().unwrap().contains("config.toml"),
            "turn args_preview must contain the path argument"
        );
        assert_eq!(
            trace["events"][0]["args_preview"], "",
            "session_started args_preview must be empty"
        );
        assert_eq!(
            trace["events"][3]["args_preview"], "",
            "session_finished args_preview must be empty"
        );
    }

    #[test]
    fn session_report_redacts_sensitive_token_in_task_and_summary() {
        let repo = tempfile::TempDir::new().unwrap();
        let repo_path = repo.path().to_str().unwrap();
        let mut agent = test_agent(repo_path);
        agent.session_init();

        let task = "Run with api_key=hunter2 and check output";
        let summary = "Done. Used password=s3cr3t internally.";

        let artifacts = agent
            .session_finish(
                task,
                summary,
                StopReason::Success,
                0,
                std::time::Instant::now(),
                "2024-01-01 00:00:00",
            )
            .expect("top-level session should produce artifacts");

        let report = std::fs::read_to_string(&artifacts.log_path).unwrap();
        assert!(!report.contains("hunter2"), "report must not contain raw api_key value");
        assert!(!report.contains("s3cr3t"), "report must not contain raw password value");
        assert!(report.contains("[redacted]"));
    }

    #[test]
    fn session_trace_redacts_sensitive_token_in_task_and_summary_preview() {
        let repo = tempfile::TempDir::new().unwrap();
        let repo_path = repo.path().to_str().unwrap();
        let mut agent = test_agent(repo_path);
        agent.session_init();

        let task = "Deploy with secret=topsecret123";
        let summary = "Completed successfully.";

        let artifacts = agent
            .session_finish(
                task,
                summary,
                StopReason::Success,
                0,
                std::time::Instant::now(),
                "2024-01-01 00:00:00",
            )
            .expect("top-level session should produce artifacts");

        let trace_path = artifacts.trace_path.expect("trace path should exist");
        let trace_raw = std::fs::read_to_string(trace_path).unwrap();
        assert!(!trace_raw.contains("topsecret123"), "trace must not contain raw secret value");
        assert!(trace_raw.contains("[redacted]"));
    }

    #[test]
    fn trace_event_detail_redacts_sensitive_token_in_turn_output() {
        let repo = tempfile::TempDir::new().unwrap();
        let repo_path = repo.path().to_str().unwrap();
        let mut agent = test_agent(repo_path);
        agent.session_init();

        agent.history_mut().push(Turn {
            step: 1,
            thought: "Reading config".to_string(),
            tool: "read_file".to_string(),
            args: serde_json::json!({ "path": "config.toml" }),
            output: "host=localhost\napi_key=ultraSecret99\nport=8080".to_string(),
            success: true,
        });

        let artifacts = agent
            .session_finish(
                "Read config file",
                "Done.",
                StopReason::Success,
                1,
                std::time::Instant::now(),
                "2024-01-01 00:00:00",
            )
            .expect("top-level session should produce artifacts");

        let trace_path = artifacts.trace_path.expect("trace path should exist");
        let trace_raw = std::fs::read_to_string(trace_path).unwrap();

        assert!(
            !trace_raw.contains("ultraSecret99"),
            "trace event detail must not contain raw api_key value; got: {trace_raw}"
        );
        assert!(
            trace_raw.contains("[redacted]"),
            "trace must contain redaction marker"
        );
    }

    #[test]
    fn trace_event_detail_redacts_sensitive_token_in_turn_thought() {
        let repo = tempfile::TempDir::new().unwrap();
        let repo_path = repo.path().to_str().unwrap();
        let mut agent = test_agent(repo_path);
        agent.session_init();

        agent.history_mut().push(Turn {
            step: 1,
            thought: "Found password=letmein in env, proceeding.".to_string(),
            tool: "run_command".to_string(),
            args: serde_json::json!({ "program": "env" }),
            output: "ok".to_string(),
            success: true,
        });

        let artifacts = agent
            .session_finish(
                "Check environment",
                "Done.",
                StopReason::Success,
                1,
                std::time::Instant::now(),
                "2024-01-01 00:00:00",
            )
            .expect("top-level session should produce artifacts");

        let trace_path = artifacts.trace_path.expect("trace path should exist");
        let trace_raw = std::fs::read_to_string(trace_path).unwrap();

        assert!(
            !trace_raw.contains("letmein"),
            "trace event detail must not contain raw password value; got: {trace_raw}"
        );
        assert!(
            trace_raw.contains("[redacted]"),
            "trace must contain redaction marker"
        );
    }

    #[test]
    fn restore_task_state_clears_session_local_signals() {
        let repo = tempfile::TempDir::new().unwrap();
        let mut agent = test_agent(repo.path().to_str().unwrap());

        let stale_json = r#"{
  "goal": "fix the auth bug",
  "attempted_actions": ["step 1: read_file (path=src/auth.rs)"],
  "artifacts_found": ["file touched: src/auth.rs"],
  "blocked_on": [],
  "repeated_signatures": [],
  "recent_signatures": [
    "read_file (path=a.rs) -> line 1",
    "list_dir () -> src/",
    "search_in_files (pattern=auth) -> src/auth.rs:1",
    "find_files (pattern=*.rs) -> src/auth.rs"
  ],
  "recent_progress_markers": [
    "exploration",
    "exploration",
    "exploration",
    "exploration"
  ],
  "next_best_action": "Run focused verification before broader edits."
}"#;

        let path = agent.task_state_path();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, stale_json).unwrap();

        let restored = agent.restore_task_state_from_disk();
        assert!(restored, "restore should succeed for a resume-worthy state");

        assert_eq!(agent.task_state().goal(), Some("fix the auth bug"));
        assert_eq!(
            agent.task_state().next_best_action_hint(),
            Some("Run focused verification before broader edits.")
        );

        assert!(
            !agent.task_state().has_recent_stall_pressure(),
            "stall pressure must be cleared after restore"
        );
        assert!(
            !agent.task_state().strategy_change_required(),
            "strategy_change_required must be false after restore"
        );
        assert!(
            !agent.task_state().has_exploration_pressure(),
            "exploration pressure must be cleared after restore"
        );
    }

    #[test]
    fn resume_guidance_warns_about_stale_plan_file_when_present() {
        let repo = tempfile::TempDir::new().unwrap();
        let mut agent = test_agent(repo.path().to_str().unwrap());
        agent.set_resumed_from_task_state(true);
        std::fs::write(repo.path().join("PLAN.md"), "# Plan\n- step 1\n").unwrap();
        agent.task_state_mut().set_goal("fix the parser");

        let guidance = agent.resume_guidance().expect("guidance must be present");
        assert!(
            guidance.contains("PLAN.md"),
            "guidance must mention the plan file path: {guidance}"
        );
        assert!(
            guidance.contains("Verify it still reflects"),
            "guidance must warn to verify the plan: {guidance}"
        );
    }

    #[test]
    fn resume_guidance_warns_about_ai_plan_file_when_present() {
        let repo = tempfile::TempDir::new().unwrap();
        let mut agent = test_agent(repo.path().to_str().unwrap());
        agent.set_resumed_from_task_state(true);
        std::fs::create_dir_all(repo.path().join(".ai")).unwrap();
        std::fs::write(repo.path().join(".ai/plan.md"), "# Plan\n").unwrap();
        agent.task_state_mut().set_goal("fix the parser");

        let guidance = agent.resume_guidance().expect("guidance must be present");
        assert!(
            guidance.contains(".ai/plan.md"),
            "guidance must mention .ai/plan.md: {guidance}"
        );
    }

    #[test]
    fn resume_guidance_omits_plan_warning_when_no_plan_file() {
        let repo = tempfile::TempDir::new().unwrap();
        let mut agent = test_agent(repo.path().to_str().unwrap());
        agent.set_resumed_from_task_state(true);
        agent.task_state_mut().set_goal("fix the parser");

        let guidance = agent.resume_guidance().expect("guidance must be present");
        assert!(
            !guidance.contains("plan file exists"),
            "no plan warning when no file present: {guidance}"
        );
    }

    #[test]
    fn resume_guidance_omits_plan_warning_when_not_resumed() {
        let repo = tempfile::TempDir::new().unwrap();
        let agent = test_agent(repo.path().to_str().unwrap());
        std::fs::write(repo.path().join("PLAN.md"), "# Plan\n").unwrap();

        assert!(
            agent.resume_guidance().is_none(),
            "resume_guidance must return None when session is not resumed"
        );
    }

    #[test]
    fn session_finish_report_includes_decisions_section_when_present() {
        let repo = tempfile::TempDir::new().unwrap();
        let repo_path = repo.path().to_str().unwrap();
        let mut agent = test_agent(repo_path);
        agent.session_init();

        // Pre-write a session_decisions.md as append_decision would
        let state_dir = repo.path().join(".ai").join("state");
        std::fs::create_dir_all(&state_dir).unwrap();
        std::fs::write(
            state_dir.join("session_decisions.md"),
            "\n## Step 2 - 2024-01-01 00:00:00 [developer]\nTool: str_replace\nDecision: str_replace preferred over write_file for targeted edit\n",
        ).unwrap();

        agent.history_mut().push(crate::history::Turn {
            step: 1,
            thought: "Edit config".to_string(),
            tool: "str_replace".to_string(),
            args: serde_json::json!({ "path": "config.toml" }),
            output: "ok".to_string(),
            success: true,
        });

        let artifacts = agent
            .session_finish(
                "Targeted edit",
                "done",
                StopReason::Success,
                1,
                std::time::Instant::now(),
                "2024-01-01 00:00:00",
            )
            .expect("top-level session should produce artifacts");

        let report = std::fs::read_to_string(artifacts.log_path).unwrap();
        assert!(report.contains("## Decisions"), "report must include Decisions section");
        assert!(
            report.contains("str_replace preferred over write_file"),
            "report must include decision text"
        );
    }

    #[test]
    fn session_finish_report_omits_decisions_section_when_absent() {
        let repo = tempfile::TempDir::new().unwrap();
        let repo_path = repo.path().to_str().unwrap();
        let mut agent = test_agent(repo_path);
        agent.session_init();

        agent.history_mut().push(crate::history::Turn {
            step: 1,
            thought: "Read config".to_string(),
            tool: "read_file".to_string(),
            args: serde_json::json!({ "path": "config.toml" }),
            output: "ok".to_string(),
            success: true,
        });

        let artifacts = agent
            .session_finish(
                "Inspect config",
                "done",
                StopReason::Success,
                1,
                std::time::Instant::now(),
                "2024-01-01 00:00:00",
            )
            .expect("top-level session should produce artifacts");

        let report = std::fs::read_to_string(artifacts.log_path).unwrap();
        assert!(
            !report.contains("## Decisions"),
            "report must NOT include Decisions section when no decisions were recorded"
        );
    }

    #[test]
    fn find_stale_plan_file_prefers_ai_plan_over_root_plan() {
        let repo = tempfile::TempDir::new().unwrap();
        let mut agent = test_agent(repo.path().to_str().unwrap());
        agent.set_resumed_from_task_state(true);
        std::fs::create_dir_all(repo.path().join(".ai")).unwrap();
        std::fs::write(repo.path().join(".ai/plan.md"), "plan a").unwrap();
        std::fs::write(repo.path().join("PLAN.md"), "plan b").unwrap();

        let found = agent.find_stale_plan_file();
        assert_eq!(found.as_deref(), Some(".ai/plan.md"));
    }

    #[test]
    fn find_stale_plan_file_finds_canonical_current_plan() {
        // .ai/state/current_plan.md is the canonical path written by memory_write("plan").
        // It must be found even when no legacy plan files exist.
        let repo = tempfile::TempDir::new().unwrap();
        let mut agent = test_agent(repo.path().to_str().unwrap());
        agent.set_resumed_from_task_state(true);
        let state_dir = repo.path().join(".ai").join("state");
        std::fs::create_dir_all(&state_dir).unwrap();
        std::fs::write(state_dir.join("current_plan.md"), "# Plan\n- step 1\n").unwrap();

        let found = agent.find_stale_plan_file();
        assert_eq!(
            found.as_deref(),
            Some(".ai/state/current_plan.md"),
            "canonical plan must be found"
        );
    }

    #[test]
    fn find_stale_plan_file_prefers_canonical_over_legacy() {
        // When both .ai/state/current_plan.md and a legacy file exist, the canonical
        // path must be returned first — it is checked first in the candidates array.
        let repo = tempfile::TempDir::new().unwrap();
        let mut agent = test_agent(repo.path().to_str().unwrap());
        agent.set_resumed_from_task_state(true);
        let state_dir = repo.path().join(".ai").join("state");
        std::fs::create_dir_all(&state_dir).unwrap();
        std::fs::write(state_dir.join("current_plan.md"), "# Canonical plan").unwrap();
        std::fs::write(repo.path().join("PLAN.md"), "# Legacy plan").unwrap();

        let found = agent.find_stale_plan_file();
        assert_eq!(
            found.as_deref(),
            Some(".ai/state/current_plan.md"),
            "canonical path must take priority over legacy PLAN.md"
        );
    }

    #[test]
    fn session_finish_removes_old_log_files_older_than_30_days() {
        let repo = tempfile::TempDir::new().unwrap();
        let repo_path = repo.path().to_str().unwrap();
        let mut agent = test_agent(repo_path);
        agent.session_init();

        // Create a stale .log file (older than 30 days) in the logs directory.
        let logs_dir = repo.path().join(".ai").join("logs");
        std::fs::create_dir_all(&logs_dir).unwrap();
        let stale_log = logs_dir.join("background-old.log");
        std::fs::write(&stale_log, "old log content").unwrap();
        let stale_mtime = filetime::FileTime::from_system_time(
            std::time::SystemTime::now() - std::time::Duration::from_secs(32 * 24 * 60 * 60),
        );
        filetime::set_file_mtime(&stale_log, stale_mtime).unwrap();

        agent.session_finish(
            "cleanup test",
            "done",
            StopReason::Success,
            0,
            std::time::Instant::now(),
            "2024-01-01 00:00:00",
        );

        assert!(
            !stale_log.exists(),
            "stale .log file must be removed by session_finish cleanup"
        );
    }

    #[test]
    fn session_finish_cleans_stale_pid_files_when_background_group_enabled() {
        let repo = tempfile::TempDir::new().unwrap();
        let repo_path = repo.path().to_str().unwrap();

        // Create agent with background group enabled.
        let cfg = AgentConfig {
            tool_groups: vec!["background".to_string()],
            ..AgentConfig::default()
        };
        let mut agent = SweAgent::new(cfg, repo_path, 5, Role::Developer).unwrap();
        agent.session_init();

        // Create a stale .pid file in the state directory.
        let state_dir = repo.path().join(".ai").join("state");
        std::fs::create_dir_all(&state_dir).unwrap();
        let stale_pid = state_dir.join("proc-old.pid");
        std::fs::write(&stale_pid, "99999").unwrap();
        let stale_mtime = filetime::FileTime::from_system_time(
            std::time::SystemTime::now() - std::time::Duration::from_secs(2 * 24 * 60 * 60),
        );
        filetime::set_file_mtime(&stale_pid, stale_mtime).unwrap();

        agent.session_finish(
            "background cleanup test",
            "done",
            StopReason::Success,
            0,
            std::time::Instant::now(),
            "2024-01-01 00:00:00",
        );

        assert!(
            !stale_pid.exists(),
            "stale .pid file must be removed when background group is enabled"
        );
    }
}
