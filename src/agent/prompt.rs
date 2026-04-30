use crate::agent::core::SweAgent;
use crate::agent::tools::first_line;
use crate::history::Turn;
use crate::loop_policy::is_exploration_tool;
use crate::tools::{ToolStatus, all_tool_specs, find_tool_spec, tool_status};
use crate::tools::spec::RUN_SCRIPT_GUIDE;

impl SweAgent {
    pub(crate) fn build_prompt(&self, task: &str, step: usize) -> String {
        let boss_notes = self.cached_boss_notes();
        let user_profile = self.cached_user_profile();
        let mut prompt = String::new();

        if !boss_notes.is_empty() {
            prompt.push_str("### Boss Notes\n\n");
            prompt.push_str(boss_notes);
            prompt.push_str("\n\n");
        }
        if !user_profile.is_empty() {
            prompt.push_str("### User Profile\n\n");
            prompt.push_str(user_profile);
            prompt.push_str("\n\n");
        }

        prompt.push_str("### Task\n\n");
        prompt.push_str(task);
        prompt.push_str("\n\n");

        if let Some(task_source) = self.task_source() {
            prompt.push_str("### Authoritative Task Source\n\n");
            prompt.push_str(&format!(
                "- The user's explicit task input came from file: {task_source}\n- Treat this file as the authoritative requirements source for the current session.\n- Do not prefer older memory entries or invent unrelated `knowledge/*` keys before processing this task source.\n\n"
            ));
        }

        prompt.push_str("### Working Memory\n\n");
        prompt.push_str(&self.task_state().format_for_prompt());
        prompt.push_str("\n\n");

        if step == 1 {
            if let Some(resume_guidance) = self.resume_guidance() {
                prompt.push_str("### Resume Guidance\n\n");
                prompt.push_str(&resume_guidance);
                prompt.push_str("\n\n");
            }
        }

        if step > 1 {
            prompt.push_str("### History\n\n");
            prompt.push_str(&self.history().format(self.max_output_chars()));
            prompt.push_str("\n\n");
        }

        if let Some(notes) = self.build_strategy_notes() {
            prompt.push_str("### Strategy Notes\n\n");
            prompt.push_str(&notes);
            prompt.push_str("\n\n");
        }

        prompt.push_str("### Project Context\n\n");
        if let Some(proj) = crate::agent::tools::detect_project_name(self.root()) {
            prompt.push_str(&format!("Project: {}\n", proj));
        }
        if let Some(repo) = crate::agent::tools::detect_github_repo(self.root()) {
            prompt.push_str(&format!("GitHub: {}\n", repo));
        }
        prompt.push_str(&format!("Root: {}\n", self.root().display()));
        prompt.push_str(&format!("Role: {}\n", self.role().name()));
        prompt.push_str(&format!(
            "Step: {} of {} (depth: {})\n",
            step,
            self.max_steps(),
            self.depth()
        ));

        // Inject the Rhai scripting guide for roles that have run_script.
        // This is the primary source of truth about syntax, host functions,
        // limits, and usage patterns. Without it the agent only sees the
        // one-line prompt_line and produces incorrect scripts.
        let effective_tools = self
            .role()
            .allowed_tools_with_groups(&self.cfg_snapshot().tool_groups);
        let role_has_run_script = effective_tools.contains(&"run_script")
            || self.role().name() == "default"; // unrestricted role
        if role_has_run_script {
            prompt.push_str("\n");
            prompt.push_str(RUN_SCRIPT_GUIDE);
            prompt.push('\n');
        }

        prompt
    }

    pub(crate) fn build_strategy_notes(&self) -> Option<String> {
        let recent = self.history().recent_turns(6);
        if recent.is_empty() {
            return None;
        }

        let role_name = self.role().name();
        let allowed = self
            .role()
            .allowed_tools_with_groups(&self.cfg_snapshot().tool_groups);
        let unrestricted = allowed.is_empty();
        let mut notes = Vec::new();
        let mut seen_signatures = Vec::new();
        let has_stall_pressure = self.task_state().has_recent_stall_pressure();

        if has_stall_pressure {
            notes.push(
                "- Recent stall pressure detected: at least 3 of the last 4 steps were exploration-heavy without meaningful implementation or verification progress. The next step must narrow to implementation, verification, diff inspection, clarification, or finish with a blocker."
                    .to_string(),
            );
        } else if self.task_state().has_exploration_pressure() {
            let note = if self.task_state().has_repeated_exploration_pressure() {
                "- Recent steps are dominated by repeated exploration: 3+ consecutive exploration steps with repeated signatures detected. Do not spend the next step on another broad read/list/search call unless it narrows to a clearly different target. Prefer implementation, verification, `diff_repo`, `run_command`, or `finish` with a blocker."
            } else {
                "- Sustained exploration pressure: 3+ consecutive exploration steps detected. You already have enough context to stop gathering broadly and take a narrower implementation, verification, diff-inspection, clarification, or blocker-reporting step."
            };
            notes.push(note.to_string());
        }

        if self.task_state().strategy_change_required() {
            let note = if has_stall_pressure {
                "- Strategy change required: The exploration budget is exhausted (4+ consecutive exploration steps) and recent stall pressure is high. The next step must change strategy: implement, verify, inspect a diff, ask for clarification, or finish with a blocker. Continuing with another exploration step will trigger a no-progress bailout."
            } else {
                "- Strategy change required: The exploration budget is exhausted (4+ consecutive exploration steps). The next step must change strategy: implement, verify, inspect a diff, ask for clarification, or finish with a blocker. Continuing with another exploration-only step will trigger a no-progress bailout."
            };
            notes.push(note.to_string());
        }

        if let Some(loop_note) = emerging_loop_note(&self.history().turns) {
            notes.push(loop_note);
        }

        if role_name == "boss" && self.boss_coordination_loop_pressure() {
            notes.push(
                "- Boss is coordinating without converging. Stop bouncing between `memory_read`, `memory_write`, and repeated delegations. Delegate one end-to-end subtask with explicit success criteria, then either do a single verification pass or call `finish` if the user request is already satisfied."
                    .to_string(),
            );
        }

        for turn in recent.iter().rev() {
            let Some(spec) = find_tool_spec(&turn.tool) else {
                continue;
            };
            if !matches!(spec.status, ToolStatus::Stub | ToolStatus::Experimental) {
                continue;
            }

            let signature = format!(
                "{}|{}|{}",
                turn.tool,
                turn.args,
                first_line(&turn.output, 80)
            );
            if seen_signatures.contains(&signature) {
                continue;
            }
            seen_signatures.push(signature);

            let repeat_count = recent
                .iter()
                .filter(|other| {
                    other.tool == turn.tool
                        && other.args == turn.args
                        && first_line(&other.output, 80) == first_line(&turn.output, 80)
                })
                .count();

            let threshold = match spec.status {
                ToolStatus::Stub => 2,
                ToolStatus::Experimental => 2,
                ToolStatus::Real => continue,
            };

            if repeat_count < threshold {
                continue;
            }

            let status_label = match spec.status {
                ToolStatus::Stub => "limited",
                ToolStatus::Experimental => "experimental",
                ToolStatus::Real => unreachable!(),
            };

            let mut note = format!(
                "- `{}` is {} and already returned the same result {} times with the same args. Do not call it again unchanged in the next step.",
                turn.tool, status_label, repeat_count
            );

            let fallbacks =
                suggested_fallbacks(spec.canonical_name, role_name, unrestricted, &allowed);
            if !fallbacks.is_empty() {
                note.push_str(" Prefer more reliable alternatives such as ");
                note.push_str(
                    &fallbacks
                        .iter()
                        .map(|tool| format!("`{tool}`"))
                        .collect::<Vec<_>>()
                        .join(", "),
                );
                note.push('.');
            } else if !turn.success {
                note.push_str(
                    " Choose a different real tool or finish with a blocker if no real fallback exists.",
                );
            }

            notes.push(note);
        }

        if notes.is_empty() {
            None
        } else {
            Some(notes.join("\n"))
        }
    }

    pub(crate) fn detect_loop(&self) -> bool {
        if self.history().turns.len() < 4 {
            return false;
        }

        if self.role().name() == "boss" && self.boss_coordination_loop_pressure() {
            return true;
        }

        if self.task_state().has_recent_stall_pressure() {
            let last4 = self
                .history()
                .turns
                .iter()
                .rev()
                .take(4)
                .collect::<Vec<_>>();
            let exploration_heavy = last4.len() == 4
                && last4
                    .iter()
                    .filter(|t| is_exploration_tool(t.tool.as_str()))
                    .count()
                    >= 3
                && !last4.iter().any(|t| is_meaningful_progress_turn(t));
            if exploration_heavy {
                return true;
            }
        }

        if self.task_state().has_repeated_exploration_pressure() {
            let last3 = self
                .history()
                .turns
                .iter()
                .rev()
                .take(3)
                .collect::<Vec<_>>();
            let all_exploration =
                last3.len() == 3 && last3.iter().all(|t| is_exploration_tool(t.tool.as_str()));
            if all_exploration {
                return true;
            }
        }

        if self.task_state().strategy_change_required() {
            let last4 = self
                .history()
                .turns
                .iter()
                .rev()
                .take(4)
                .collect::<Vec<_>>();
            let all_exploration =
                last4.len() == 4 && last4.iter().all(|t| is_exploration_tool(t.tool.as_str()));
            if all_exploration {
                return true;
            }
        }

        if self.history().turns.len() >= 2 {
            let last2 = self
                .history()
                .turns
                .iter()
                .rev()
                .take(2)
                .collect::<Vec<_>>();
            let limited_repeat = last2.len() == 2
                && last2[0].tool == last2[1].tool
                && last2[0].args == last2[1].args
                && first_line(&last2[0].output, 80) == first_line(&last2[1].output, 80)
                && matches!(tool_status(&last2[0].tool), Some(ToolStatus::Stub));
            if limited_repeat {
                return true;
            }
        }

        if self.history().turns.len() >= 3 {
            let last3 = self
                .history()
                .turns
                .iter()
                .rev()
                .take(3)
                .collect::<Vec<_>>();
            let experimental_repeat = last3.len() == 3
                && last3[0].tool == last3[1].tool
                && last3[1].tool == last3[2].tool
                && last3[0].args == last3[1].args
                && last3[1].args == last3[2].args
                && first_line(&last3[0].output, 80) == first_line(&last3[1].output, 80)
                && first_line(&last3[1].output, 80) == first_line(&last3[2].output, 80)
                && matches!(tool_status(&last3[0].tool), Some(ToolStatus::Experimental));
            if experimental_repeat {
                return true;
            }
        }

        let last4 = self
            .history()
            .turns
            .iter()
            .rev()
            .take(4)
            .collect::<Vec<_>>();
        matches_structural_loop_pattern(&last4)
    }

    fn boss_coordination_loop_pressure(&self) -> bool {
        let turns: Vec<_> = self
            .history()
            .turns
            .iter()
            .filter(|t| t.step > 0)
            .rev()
            .take(6)
            .collect();
        if turns.len() < 6 {
            return false;
        }

        let orchestration_tools = [
            "memory_read",
            "memory_write",
            "spawn_agent",
            "spawn_agents",
            "notify",
            "ask_human",
            "project_map",
            "tree",
        ];

        if !turns
            .iter()
            .all(|turn| orchestration_tools.contains(&turn.tool.as_str()))
        {
            return false;
        }

        let tool_arg_sigs: Vec<String> = turns
            .iter()
            .map(|turn| format!("{}|{}", turn.tool, turn.args))
            .collect();

        let mut counts = std::collections::HashMap::new();
        for sig in &tool_arg_sigs {
            *counts.entry(sig).or_insert(0usize) += 1;
        }
        counts.values().any(|&n| n >= 3)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LoopPattern {
    AllSame,
    Alternating,
}

fn is_meaningful_progress_turn(turn: &Turn) -> bool {
    if !turn.success {
        return false;
    }

    matches!(
        turn.tool.as_str(),
        "write_file" | "str_replace" | "run_command" | "diff_repo" | "git_status" | "git_log"
    )
}

fn suggested_fallbacks(
    canonical_tool: &str,
    role_name: &str,
    unrestricted: bool,
    allowed: &[&'static str],
) -> Vec<&'static str> {
    // A candidate tool is considered "accessible" to this role/config when:
    //   - the role has no restrictions (unrestricted), OR
    //   - the candidate's allowed_roles includes this role, AND
    //   - the candidate is actually in the effective allowed list (respects
    //     optional groups: a Browser tool is only accessible when the browser
    //     group is enabled in tool_groups).
    //
    // The `allowed` slice already reflects both role membership and enabled
    // groups, so checking `allowed.contains` is the authoritative test.
    // For unrestricted roles we fall back to allowed_roles membership only
    // (since allowed is empty in that case).
    let is_accessible = |candidate: &&'static crate::tools::ToolSpec| -> bool {
        if candidate.status != crate::tools::ToolStatus::Real {
            return false;
        }
        if unrestricted {
            candidate.allowed_roles.contains(&role_name)
        } else {
            // Must be both in role's allowed_roles AND in the effective
            // allowed list (i.e. its optional group, if any, is enabled).
            candidate.allowed_roles.contains(&role_name)
                && allowed.contains(&candidate.canonical_name)
        }
    };

    let same_category: Vec<&'static str> = find_tool_spec(canonical_tool)
        .map(|spec| {
            all_tool_specs()
                .iter()
                .filter(|candidate| {
                    candidate.canonical_name != canonical_tool
                        && candidate.prompt_category == spec.prompt_category
                        && is_accessible(candidate)
                })
                .map(|candidate| candidate.canonical_name)
                .take(3)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    if !same_category.is_empty() {
        return same_category;
    }

    let heuristic_candidates: &[&str] = match canonical_tool {
        "browser_action" | "browser_get_text" | "browser_navigate" | "screenshot" => &[
            "fetch_url",
            "read_file",
            "search_in_files",
            "ask_human",
            "notify",
        ],
        "test_coverage" => &["run_command", "diff_repo", "git_status"],
        "run_background" | "process_status" | "process_list" | "process_kill" => {
            &["run_command", "notify", "ask_human"]
        }
        "spawn_agent" | "spawn_agents" => &["memory_write", "notify", "ask_human"],
        _ => &[],
    };

    heuristic_candidates
        .iter()
        .copied()
        .filter(|candidate| {
            find_tool_spec(candidate)
                .map(|spec| is_accessible(&spec))
                .unwrap_or(false)
        })
        .take(3)
        .collect()
}

fn emerging_loop_note(turns: &[Turn]) -> Option<String> {
    let last3 = turns.iter().rev().take(3).collect::<Vec<_>>();
    if last3.len() != 3 || !matches_structural_loop_pattern(&last3) {
        return None;
    }

    let is_exploration = last3.iter().all(|t| is_exploration_tool(t.tool.as_str()));

    if is_exploration {
        Some("- Recent exploration steps are converging into a loop. Do not repeat the same exploration pattern on the next step. Use the context already gathered to implement, verify, inspect a diff, ask for clarification, or finish with a blocker.".to_string())
    } else {
        Some("- Recent tool selections are converging into a loop. Do not repeat the same tool/args pattern on the next step. Change strategy, verify progress, or finish with a blocker if the task is blocked.".to_string())
    }
}

fn matches_structural_loop_pattern(turns: &[&Turn]) -> bool {
    if turns.len() < 3 {
        return false;
    }
    if turns.iter().any(|t| t.tool == "error") {
        return false;
    }

    let tool_arg_sigs = turns
        .iter()
        .map(|t| format!("{}|{}", t.tool, t.args))
        .collect::<Vec<_>>();
    let Some(pattern) = detect_pattern(&tool_arg_sigs) else {
        return false;
    };

    let is_exploration = turns.iter().all(|t| is_exploration_tool(t.tool.as_str()));

    if is_exploration {
        let thoughts = turns.iter().map(|t| t.thought.as_str()).collect::<Vec<_>>();
        if !matches_pattern(&thoughts, pattern) {
            return false;
        }
    }

    let output_sigs = turns
        .iter()
        .map(|t| first_line(&t.output, 50).to_string())
        .collect::<Vec<_>>();

    if !is_exploration {
        all_same(&output_sigs)
    } else {
        matches_pattern(&output_sigs, pattern)
    }
}

fn detect_pattern<T: PartialEq>(values: &[T]) -> Option<LoopPattern> {
    if all_same(values) {
        return Some(LoopPattern::AllSame);
    }
    if alternating(values) {
        return Some(LoopPattern::Alternating);
    }
    None
}

fn matches_pattern<T: PartialEq>(values: &[T], pattern: LoopPattern) -> bool {
    match pattern {
        LoopPattern::AllSame => all_same(values),
        LoopPattern::Alternating => alternating(values),
    }
}

fn all_same<T: PartialEq>(values: &[T]) -> bool {
    if values.len() < 2 {
        return false;
    }
    values.windows(2).all(|w| w[0] == w[1])
}

fn alternating<T: PartialEq>(values: &[T]) -> bool {
    match values.len() {
        3 => values[0] == values[2] && values[0] != values[1],
        4 => values[0] == values[2] && values[1] == values[3] && values[0] != values[1],
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::SweAgent;
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

    fn record_turn(agent: &mut SweAgent, turn: Turn) {
        agent.task_state_mut().update_from_turn(&turn);
        agent.history_mut().push(turn);
    }

    #[test]
    fn strategy_notes_warn_before_general_loop_trip() {
        let mut agent = test_agent(Role::Developer);
        for step in 1..=3 {
            agent.history_mut().push(Turn {
                step,
                thought: "Check diff again".to_string(),
                tool: "diff_repo".to_string(),
                args: serde_json::json!({}),
                output: "working tree clean".to_string(),
                success: true,
            });
        }

        let notes = agent
            .build_strategy_notes()
            .expect("strategy note expected");
        assert!(notes.contains("converging into a loop"));
    }

    #[test]
    fn strategy_notes_warn_before_alternating_loop_trip() {
        let mut agent = test_agent(Role::Developer);
        for (step, tool, args) in [
            (1, "read_file", serde_json::json!({ "path": "a.rs" })),
            (2, "read_file", serde_json::json!({ "path": "b.rs" })),
            (3, "read_file", serde_json::json!({ "path": "a.rs" })),
        ] {
            agent.history_mut().push(Turn {
                step,
                thought: if step % 2 == 0 {
                    "Inspect file B".to_string()
                } else {
                    "Inspect file A".to_string()
                },
                tool: tool.to_string(),
                args,
                output: format!("File read {}", if step % 2 == 0 { "B" } else { "A" }),
                success: true,
            });
        }

        let notes = agent
            .build_strategy_notes()
            .expect("strategy note expected");
        assert!(notes.contains("converging into a loop"));
    }

    #[test]
    fn strategy_notes_warn_about_recent_stall_pressure() {
        let mut agent = test_agent(Role::Developer);

        for (step, tool, args, output, success) in [
            (
                1,
                "list_dir",
                serde_json::json!({ "path": "." }),
                "src\nCargo.toml",
                true,
            ),
            (
                2,
                "read_file",
                serde_json::json!({ "path": "README.md" }),
                "File: README.md (lines 1-60 of 120)",
                true,
            ),
            (
                3,
                "browser_get_text",
                serde_json::json!({ "url": "https://example.com" }),
                "[experimental tool] browser connection is not configured",
                false,
            ),
            (
                4,
                "search_in_files",
                serde_json::json!({ "pattern": "task state", "dir": "src" }),
                "src/task_state.rs:1: use serde::{Deserialize, Serialize};",
                true,
            ),
        ] {
            record_turn(
                &mut agent,
                Turn {
                    step,
                    thought: "Still exploring".to_string(),
                    tool: tool.to_string(),
                    args,
                    output: output.to_string(),
                    success,
                },
            );
        }

        let notes = agent
            .build_strategy_notes()
            .expect("strategy note expected");
        assert!(notes.contains("stall pressure"));
        assert!(notes.contains("without meaningful implementation or verification progress"));
    }

    #[test]
    fn strategy_notes_warn_about_varied_exploration_without_repeated_signatures() {
        let mut agent = test_agent(Role::Developer);

        for (step, tool, args, output) in [
            (
                1,
                "list_dir",
                serde_json::json!({ "path": "src" }),
                "mod.rs\nprompt.rs",
            ),
            (
                2,
                "read_file",
                serde_json::json!({ "path": "src/agent/mod.rs" }),
                "pub mod core;",
            ),
            (
                3,
                "search_in_files",
                serde_json::json!({ "pattern": "StopReason", "dir": "src" }),
                "src/agent/core.rs:8: pub enum StopReason {",
            ),
            (
                4,
                "open_file_region",
                serde_json::json!({ "path": "src/agent/session.rs", "start_line": 1, "end_line": 40 }),
                "use crate::agent::core::{StopReason, SweAgent};",
            ),
        ] {
            record_turn(
                &mut agent,
                Turn {
                    step,
                    thought: "Still gathering context".to_string(),
                    tool: tool.to_string(),
                    args,
                    output: output.to_string(),
                    success: true,
                },
            );
        }

        assert!(agent.task_state().has_recent_stall_pressure());
        assert!(!agent.task_state().has_repeated_exploration_pressure());

        let notes = agent
            .build_strategy_notes()
            .expect("strategy note expected");
        assert!(notes.contains("stall pressure"));
        assert!(!notes.contains("dominated by repeated exploration"));
    }

    #[test]
    fn detect_loop_trips_for_recent_stall_pressure() {
        let mut agent = test_agent(Role::Developer);

        for (step, tool, args, output, success) in [
            (
                1,
                "list_dir",
                serde_json::json!({ "path": "." }),
                "src\nCargo.toml",
                true,
            ),
            (
                2,
                "read_file",
                serde_json::json!({ "path": "src/lib.rs" }),
                "pub fn run() {}",
                true,
            ),
            (
                3,
                "browser_get_text",
                serde_json::json!({ "url": "https://example.com" }),
                "[experimental tool] browser connection is not configured",
                false,
            ),
            (
                4,
                "search_in_files",
                serde_json::json!({ "pattern": "run", "dir": "src" }),
                "src/lib.rs:1: pub fn run() {}",
                true,
            ),
        ] {
            record_turn(
                &mut agent,
                Turn {
                    step,
                    thought: "Still orienting".to_string(),
                    tool: tool.to_string(),
                    args,
                    output: output.to_string(),
                    success,
                },
            );
        }

        assert!(agent.task_state().has_recent_stall_pressure());
        assert!(agent.detect_loop());
    }

    #[test]
    fn detect_loop_trips_for_varied_exploration_without_repeated_signatures() {
        let mut agent = test_agent(Role::Developer);

        for (step, tool, args, output) in [
            (
                1,
                "list_dir",
                serde_json::json!({ "path": "src/agent" }),
                "core.rs\nprompt.rs\nsession.rs",
            ),
            (
                2,
                "read_file",
                serde_json::json!({ "path": "src/task_state.rs" }),
                "use serde::{Deserialize, Serialize};",
            ),
            (
                3,
                "search_in_files",
                serde_json::json!({ "pattern": "exploration", "dir": "src" }),
                "src/task_state.rs:14: recent_progress_markers: Vec<String>,",
            ),
            (
                4,
                "outline",
                serde_json::json!({ "path": "src/agent/prompt.rs" }),
                "build_prompt\nbuild_strategy_notes\ndetect_loop",
            ),
        ] {
            record_turn(
                &mut agent,
                Turn {
                    step,
                    thought: "Still orienting".to_string(),
                    tool: tool.to_string(),
                    args,
                    output: output.to_string(),
                    success: true,
                },
            );
        }

        assert!(agent.task_state().has_recent_stall_pressure());
        assert!(!agent.task_state().has_repeated_exploration_pressure());
        assert!(agent.detect_loop());
    }

    #[test]
    fn detect_loop_does_not_trip_when_recent_verification_progress_exists() {
        let mut agent = test_agent(Role::Developer);

        for (step, tool, args, output, success) in [
            (
                1,
                "list_dir",
                serde_json::json!({ "path": "." }),
                "src\nCargo.toml",
                true,
            ),
            (
                2,
                "read_file",
                serde_json::json!({ "path": "src/lib.rs" }),
                "pub fn run() {}",
                true,
            ),
            (
                3,
                "run_command",
                serde_json::json!({ "program": "cargo test" }),
                "test result: ok. 12 passed;",
                true,
            ),
            (
                4,
                "search_in_files",
                serde_json::json!({ "pattern": "run", "dir": "src" }),
                "src/lib.rs:1: pub fn run() {}",
                true,
            ),
        ] {
            record_turn(
                &mut agent,
                Turn {
                    step,
                    thought: "Making progress".to_string(),
                    tool: tool.to_string(),
                    args,
                    output: output.to_string(),
                    success,
                },
            );
        }

        assert!(!agent.task_state().has_recent_stall_pressure());
        assert!(!agent.detect_loop());
    }

    #[test]
    fn build_prompt_includes_scripting_guide_for_roles_with_run_script() {
        // developer, navigator, qa have run_script — guide must appear.
        for (role, name) in [
            (Role::Developer, "developer"),
            (Role::Navigator, "navigator"),
            (Role::Qa, "qa"),
        ] {
            let agent = test_agent(role);
            let prompt = agent.build_prompt("Count lines", 1);
            assert!(
                prompt.contains("read_lines"),
                "role {name}: scripting guide must be injected into prompt"
            );
            assert!(
                prompt.contains("Available host functions"),
                "role {name}: guide must include host functions section"
            );
        }
    }

    #[test]
    fn build_prompt_excludes_scripting_guide_for_roles_without_run_script() {
        // boss, reviewer, research do NOT have run_script — guide must not appear.
        for (role, name) in [
            (Role::Boss, "boss"),
            (Role::Reviewer, "reviewer"),
            (Role::Research, "research"),
        ] {
            let agent = test_agent(role);
            let prompt = agent.build_prompt("Plan the feature", 1);
            assert!(
                !prompt.contains("Available host functions"),
                "role {name}: scripting guide must NOT be injected"
            );
        }
    }

    #[test]
    fn build_prompt_step1_without_resume_state_has_no_resume_guidance() {
        // When the agent has not been resumed from persisted task state,
        // build_prompt at step 1 must not include a Resume Guidance section.
        // Guards against regressions where resume_guidance() always returns Some.
        let agent = test_agent(Role::Developer);
        let prompt = agent.build_prompt("Fix the parser", 1);
        assert!(!prompt.contains("### Resume Guidance"));
        assert!(prompt.contains("### Task"));
        assert!(prompt.contains("Fix the parser"));
    }

    #[test]
    fn build_prompt_step2_includes_history_and_no_resume_guidance() {
        // At step > 1 the History section must appear and Resume Guidance must
        // not appear regardless of agent state. Guards against off-by-one
        // regressions in the step == 1 / step > 1 branching inside build_prompt.
        let mut agent = test_agent(Role::Developer);
        agent.history_mut().push(Turn {
            step: 1,
            thought: "Inspect the repo".to_string(),
            tool: "list_dir".to_string(),
            args: serde_json::json!({ "path": "." }),
            output: "src\nCargo.toml".to_string(),
            success: true,
        });

        let prompt = agent.build_prompt("Fix the parser", 2);
        assert!(prompt.contains("### History"));
        assert!(!prompt.contains("### Resume Guidance"));
        assert!(prompt.contains("### Task"));
    }

    /// Regression: after adding check_awp_server (Real, Browser group) to spec.rs,
    /// suggested_fallbacks for browser tools incorrectly returned only check_awp_server
    /// (same category, Real) when the browser group was not enabled — bypassing the
    /// heuristic fallback list that includes ask_human/notify.
    ///
    /// The fix: same_category candidates must be actually accessible (i.e. present in
    /// the effective allowed list), not just role-members. Group tools are only
    /// accessible when their group is enabled in tool_groups.
    #[test]
    fn strategy_notes_browser_fallbacks_exclude_disabled_group_tools() {
        // Boss without browser group enabled: check_awp_server must NOT appear
        // as a fallback for browser_get_text (it's in the disabled Browser group).
        // ask_human or notify must appear instead (both are core Boss tools).
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
        assert!(
            notes.contains("`ask_human`") || notes.contains("`notify`"),
            "expected ask_human or notify in fallbacks, got: {notes}"
        );
        assert!(
            !notes.contains("`check_awp_server`"),
            "check_awp_server must not appear when browser group is disabled, got: {notes}"
        );
    }
}
