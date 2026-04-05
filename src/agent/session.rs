use crate::agent::core::{StopReason, SweAgent};
use crate::agent::loops::SessionArtifacts;
use crate::config_loader::{global_boss_notes_path, global_user_profile_path};
use crate::config_struct::{AI_DIR, LOGS_DIR, STATE_DIR};
use crate::history::Turn;
use crate::redaction;
use crate::tools;
use serde::Serialize;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TaskStatePersistenceAction {
    Save,
    Clear,
}

#[derive(Debug, Serialize)]
struct SessionTrace<'a> {
    schema_version: u32,
    session_nr: u64,
    role: &'a str,
    config_source: &'a str,
    task: &'a str,
    stop_reason: &'a str,
    started_at: &'a str,
    ended_at: String,
    max_steps: usize,
    steps_used: usize,
    resumed_from_task_state: bool,
    summary_preview: String,
    total_calls: usize,
    ok_calls: usize,
    err_calls: usize,
    tool_stats: Vec<SessionTraceToolStat<'a>>,
    path_sensitivity_stats: Vec<SessionTracePathSensitivityStat<'static>>,
    events: Vec<SessionTraceEvent<'a>>,
}

#[derive(Debug, Serialize)]
struct SessionTraceToolStat<'a> {
    tool: &'a str,
    calls: usize,
    err_calls: usize,
}

#[derive(Debug, Serialize)]
struct SessionTracePathSensitivityStat<'a> {
    category: &'a str,
    calls: usize,
}

#[derive(Debug, Serialize)]
struct SessionTraceEvent<'a> {
    event: &'static str,
    step: Option<usize>,
    tool: Option<&'a str>,
    success: Option<bool>,
    detail: String,
}

impl SweAgent {
    pub fn session_finish(
        &self,
        task: &str,
        final_summary: &str,
        stop_reason: StopReason,
        steps_used: usize,
        started_at: std::time::Instant,
        started_at_str: &str,
    ) -> Option<SessionArtifacts> {
        if self.depth() > 0 {
            return None;
        }
        let logs_dir = self.root().join(AI_DIR).join(LOGS_DIR);
        if std::fs::create_dir_all(&logs_dir).is_err() {
            return None;
        }
        let log_path = logs_dir.join(format!("session-{:03}.md", self.session_nr()));
        let trace_path = self.session_trace_path();
        let now = tools::chrono_now();
        let role = self.role().name();
        let n = self.session_nr();
        let turns: Vec<&Turn> = self.history().turns.iter().filter(|t| t.step > 0).collect();
        let total_calls = turns.len();
        let ok_calls = turns.iter().filter(|t| t.success).count();
        let err_calls = total_calls - ok_calls;
        let mut tool_counts: std::collections::HashMap<&str, (usize, usize)> = Default::default();
        for t in &turns {
            let e = tool_counts.entry(t.tool.as_str()).or_insert((0, 0));
            e.0 += 1;
            if !t.success {
                e.1 += 1;
            }
        }
        let mut tool_list: Vec<_> = tool_counts.iter().collect();
        tool_list.sort_by(|a, b| b.1.0.cmp(&a.1.0));
        let tools_section: String = tool_list
            .iter()
            .map(|(name, (calls, errs))| {
                if *errs > 0 {
                    format!("  - {name}: {calls} calls ({errs} errors)")
                } else {
                    format!("  - {name}: {calls} calls")
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        let path_sensitivity_stats = Self::trace_path_sensitivity_stats(&turns);
        let path_sensitivity_section =
            Self::render_path_sensitivity_report_section(&path_sensitivity_stats);
        let status = match stop_reason {
            StopReason::Success => "✓ success",
            StopReason::MaxSteps => "✗ stopped: max steps reached",
            StopReason::NoProgress => "✗ stopped: no progress",
            StopReason::Error => "✗ failed / incomplete",
        };
        // Redact sensitive tokens from task and summary before writing to any artifact.
        let task_safe = redaction::redact(task);
        let summary_safe = redaction::redact(final_summary);
        let report = format!(
            "# Session #{n} — {date}\n\n**Role:** {role}  \n**Config source:** {config_source}  \n**Status:** {status}  \n**Steps used:** {steps_used}  \n**Tool calls:** {total_calls} ({ok_calls} ok, {err_calls} errors)  \n\n## Task\n\n{task_safe}\n\n## Summary\n\n{summary_safe}\n\n## Tools used\n\n{tools_section}{path_sensitivity_section}\n",
            date = now,
            config_source = self.config_source()
        );
        let _ = std::fs::write(&log_path, &report);
        let trace_path = self.write_session_trace(
            trace_path,
            &task_safe,
            &summary_safe,
            stop_reason,
            steps_used,
            started_at_str,
            total_calls,
            ok_calls,
            err_calls,
            &turns,
        );
        if !crate::tui::tui_is_active() {
            println!("  [session] Report written to {}", log_path.display());
            if let Some(trace_path) = &trace_path {
                println!("  [session] Trace written to {}", trace_path.display());
            }
        }
        self.apply_task_state_persistence(stop_reason);
        self.update_last_session(&summary_safe, &task_safe, n, stop_reason, &turns);
        Some(SessionArtifacts {
            log_path,
            trace_path,
            total_calls,
            ok_calls,
            err_calls,
            started_at,
            started_at_str: started_at_str.to_string(),
        })
    }

    fn update_last_session(
        &self,
        summary: &str,
        task: &str,
        n: u64,
        stop_reason: StopReason,
        turns: &[&Turn],
    ) {
        let state_dir = self.root().join(AI_DIR).join(STATE_DIR);
        let path = state_dir.join("last_session.md");
        let now = tools::chrono_now();
        let status_str = match stop_reason {
            StopReason::Success => "✓ success",
            StopReason::MaxSteps => "✗ max steps",
            StopReason::NoProgress => "✗ no progress",
            StopReason::Error => "✗ error",
        };
        let safety_line = Self::render_path_sensitivity_summary_line(
            &Self::trace_path_sensitivity_stats(turns),
        )
        .map(|line| format!("**Safety:** {line}\n\n"))
        .unwrap_or_default();
        let entry = format!(
            "\n## Session #{n} — {now} {status_str}\n**Task:** {task}\n\n{safety_line}{summary}\n---\n"
        );
        let existing = std::fs::read_to_string(&path).unwrap_or_default();
        let updated = format!("{existing}{entry}");
        let _ = std::fs::write(&path, &updated);
        const MAX_LINES: usize = 200;
        const KEEP_LINES: usize = 150;
        let line_count = updated.lines().count();
        if line_count > MAX_LINES {
            if !crate::tui::tui_is_active() {
                println!(
                    "  [memory] last_session.md has {line_count} lines — compressing to {KEEP_LINES}"
                );
            }
            let kept: Vec<&str> = updated
                .lines()
                .rev()
                .take(KEEP_LINES)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            let compressed = format!(
                "<!-- compressed: older entries removed, keeping last {KEEP_LINES} lines -->\n{}\n",
                kept.join("\n")
            );
            let _ = std::fs::write(&path, compressed);
        }
    }

    pub(crate) fn shutdown_tui(&mut self) {
        if let Some(mut tui) = self.take_tui() {
            tui.shutdown();
            crate::tui::set_tui_active(false);
        }
    }

    pub(crate) fn print_final_summary(
        &self,
        stop_reason: StopReason,
        summary: &str,
        steps_used: usize,
        artifacts: Option<&SessionArtifacts>,
    ) {
        for line in self.render_final_summary_lines(stop_reason, summary, steps_used, artifacts) {
            println!("{line}");
        }
    }

    fn final_summary_preview(summary: &str) -> String {
        let trimmed = summary.trim();
        if trimmed.is_empty() {
            return "(empty)".to_string();
        }

        let first_line = trimmed.lines().next().unwrap_or("").trim();
        let compact = if first_line.is_empty() {
            trimmed
        } else {
            first_line
        };
        let compact = compact.replace('\t', " ");

        if compact.len() > 220 {
            format!("{}...", &compact[..220])
        } else {
            compact
        }
    }

    pub fn session_init(&mut self) {
        let state_dir = self.root().join(AI_DIR).join(STATE_DIR);
        let counter_path = state_dir.join("session_counter.txt");
        let last_session_path = state_dir.join("last_session.md");
        let _ = std::fs::create_dir_all(&state_dir);
        self.set_resumed_from_task_state(false);
        let n = std::fs::read_to_string(&counter_path)
            .ok()
            .and_then(|s| s.trim().parse::<u64>().ok())
            .unwrap_or(0)
            + 1;
        self.set_session_nr(n);
        let _ = std::fs::write(&counter_path, n.to_string());
        if let Ok(content) = std::fs::read_to_string(&last_session_path) {
            let summary = content.lines().take(30).collect::<Vec<_>>().join("\n");
            self.history_mut().push(Turn {
                step: 0,
                thought: "Loading previous session context".to_string(),
                tool: "load_session".to_string(),
                args: serde_json::json!({}),
                output: summary,
                success: true,
            });
        }
        self.restore_task_state_from_disk();

        // Cache global files once here — build_prompt reads these every step
        // which would be N disk reads per session without caching.
        let boss_notes = global_boss_notes_path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .unwrap_or_default();
        let user_profile = global_user_profile_path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .unwrap_or_default();
        self.set_cached_boss_notes(boss_notes);
        self.set_cached_user_profile(user_profile);
    }

    pub(crate) fn task_state_path(&self) -> PathBuf {
        self.root()
            .join(AI_DIR)
            .join(STATE_DIR)
            .join("task_state.json")
    }

    fn session_trace_path(&self) -> PathBuf {
        self.root()
            .join(AI_DIR)
            .join(LOGS_DIR)
            .join(format!("session-{:03}.trace.json", self.session_nr()))
    }

    pub(crate) fn save_task_state(&self) {
        let path = self.task_state_path();
        let _ = std::fs::write(&path, self.task_state().to_json_pretty());
    }

    fn task_state_persistence_action(&self, stop_reason: StopReason) -> TaskStatePersistenceAction {
        if self.depth() > 0 || stop_reason.is_success() {
            TaskStatePersistenceAction::Clear
        } else {
            TaskStatePersistenceAction::Save
        }
    }

    fn apply_task_state_persistence(&self, stop_reason: StopReason) {
        match self.task_state_persistence_action(stop_reason) {
            TaskStatePersistenceAction::Save => self.save_task_state(),
            TaskStatePersistenceAction::Clear => self.clear_task_state(),
        }
    }

    fn clear_task_state(&self) {
        let path = self.task_state_path();
        let _ = std::fs::remove_file(&path);
    }

    fn restore_task_state_from_disk(&mut self) -> bool {
        let path = self.task_state_path();
        let Ok(content) = std::fs::read_to_string(&path) else {
            return false;
        };
        let Some(state) = crate::task_state::TaskState::from_json(&content) else {
            return false;
        };
        if !state.is_resume_worthy() {
            return false;
        }
        self.task_state_mut().clone_from(&state);
        self.task_state_mut().clear_session_signals();
        self.set_resumed_from_task_state(true);
        self.history_mut().push(Turn {
            step: 0,
            thought: "Loading persisted task state".to_string(),
            tool: "load_task_state".to_string(),
            args: serde_json::json!({ "path": path.display().to_string() }),
            output: state.format_for_prompt(),
            success: true,
        });
        true
    }

    fn write_session_trace(
        &self,
        path: PathBuf,
        task: &str,
        final_summary: &str,
        stop_reason: StopReason,
        steps_used: usize,
        started_at_str: &str,
        total_calls: usize,
        ok_calls: usize,
        err_calls: usize,
        turns: &[&Turn],
    ) -> Option<PathBuf> {
        let path_sensitivity_stats = Self::trace_path_sensitivity_stats(turns);
        let trace = SessionTrace {
            schema_version: 3,
            session_nr: self.session_nr(),
            role: self.role().name(),
            config_source: self.config_source(),
            task,
            stop_reason: self.stop_reason_label(stop_reason),
            started_at: started_at_str,
            ended_at: tools::chrono_now(),
            max_steps: self.max_steps(),
            steps_used,
            resumed_from_task_state: self.resumed_from_task_state(),
            summary_preview: Self::trace_preview(&Self::final_summary_preview(final_summary), 220),
            total_calls,
            ok_calls,
            err_calls,
            tool_stats: Self::trace_tool_stats(turns),
            path_sensitivity_stats,
            events: self.trace_events(task, stop_reason, turns),
        };

        let json = serde_json::to_string_pretty(&trace).ok()?;
        std::fs::write(&path, json).ok()?;
        Some(path)
    }

    fn trace_events<'a>(
        &'a self,
        task: &'a str,
        stop_reason: StopReason,
        turns: &[&'a Turn],
    ) -> Vec<SessionTraceEvent<'a>> {
        let mut events = Vec::with_capacity(turns.len() + 2);
        events.push(SessionTraceEvent {
            event: "session_started",
            step: None,
            tool: None,
            success: None,
            detail: Self::trace_preview(task, 160),
        });

        for turn in turns {
            let sensitivity_note = Self::trace_turn_sensitivity(turn)
                .map(|sensitivity| format!(" sensitivity={sensitivity}"))
                .unwrap_or_default();
            events.push(SessionTraceEvent {
                event: "turn",
                step: Some(turn.step),
                tool: Some(turn.tool.as_str()),
                success: Some(turn.success),
                detail: format!(
                    "thought={}{} output={}",
                    Self::trace_preview(&turn.thought, 120),
                    sensitivity_note,
                    Self::trace_preview(&turn.output, 160)
                ),
            });
        }

        events.push(SessionTraceEvent {
            event: "session_finished",
            step: turns.last().map(|turn| turn.step),
            tool: None,
            success: Some(stop_reason.is_success()),
            detail: format!(
                "stop_reason={} final_output={}",
                self.stop_reason_label(stop_reason),
                Self::trace_preview(
                    turns
                        .last()
                        .map(|turn| turn.output.as_str())
                        .unwrap_or_default(),
                    160
                )
            ),
        });
        events
    }

    fn trace_tool_stats<'a>(turns: &[&'a Turn]) -> Vec<SessionTraceToolStat<'a>> {
        let mut tool_counts: std::collections::HashMap<&str, (usize, usize)> = Default::default();
        for turn in turns {
            let entry = tool_counts.entry(turn.tool.as_str()).or_insert((0, 0));
            entry.0 += 1;
            if !turn.success {
                entry.1 += 1;
            }
        }

        let mut tool_list: Vec<_> = tool_counts
            .into_iter()
            .map(|(tool, (calls, err_calls))| SessionTraceToolStat {
                tool,
                calls,
                err_calls,
            })
            .collect();
        tool_list.sort_by(|a, b| b.calls.cmp(&a.calls).then_with(|| a.tool.cmp(b.tool)));
        tool_list
    }

    fn trace_path_sensitivity_stats(
        turns: &[&Turn],
    ) -> Vec<SessionTracePathSensitivityStat<'static>> {
        let mut counts: std::collections::HashMap<&'static str, usize> = Default::default();
        for turn in turns {
            if let Some(category) = Self::trace_turn_sensitivity(turn) {
                *counts.entry(category).or_insert(0) += 1;
            }
        }

        let mut stats: Vec<_> = counts
            .into_iter()
            .map(|(category, calls)| SessionTracePathSensitivityStat { category, calls })
            .collect();
        stats.sort_by(|a, b| {
            b.calls
                .cmp(&a.calls)
                .then_with(|| a.category.cmp(b.category))
        });
        stats
    }

    fn render_path_sensitivity_report_section(
        stats: &[SessionTracePathSensitivityStat<'static>],
    ) -> String {
        if stats.is_empty() {
            return String::new();
        }

        let body = stats
            .iter()
            .map(|stat| format!("  - {}: {} call(s)", stat.category, stat.calls))
            .collect::<Vec<_>>()
            .join("\n");
        format!("\n\n## Path sensitivity\n\n{body}")
    }

    fn render_final_summary_lines(
        &self,
        stop_reason: StopReason,
        summary: &str,
        steps_used: usize,
        artifacts: Option<&SessionArtifacts>,
    ) -> Vec<String> {
        let mut lines = vec![
            String::new(),
            format!(
                "Result : {}",
                match stop_reason {
                    StopReason::Success => "success",
                    StopReason::MaxSteps => "stopped: max steps reached",
                    StopReason::NoProgress => "stopped: no progress",
                    StopReason::Error => "failed / incomplete",
                }
            ),
            format!("Steps  : {steps_used}/{}", self.max_steps()),
        ];

        if let Some(artifacts) = artifacts {
            let elapsed = artifacts.started_at.elapsed();
            let duration_str = if elapsed.as_secs() >= 3600 {
                format!(
                    "{}h {:02}m {:02}s",
                    elapsed.as_secs() / 3600,
                    (elapsed.as_secs() % 3600) / 60,
                    elapsed.as_secs() % 60
                )
            } else if elapsed.as_secs() >= 60 {
                format!("{}m {:02}s", elapsed.as_secs() / 60, elapsed.as_secs() % 60)
            } else {
                format!("{}s", elapsed.as_secs())
            };
            lines.push(format!("Started: {}", artifacts.started_at_str));
            lines.push(format!("Ended  : {}", tools::chrono_now()));
            lines.push(format!("Time   : {duration_str}"));
            lines.push(format!(
                "Calls  : {} total ({} ok, {} errors)",
                artifacts.total_calls, artifacts.ok_calls, artifacts.err_calls
            ));
            let sensitivity_summary =
                Self::render_path_sensitivity_summary_line(&Self::trace_path_sensitivity_stats(
                    &self
                        .history()
                        .turns
                        .iter()
                        .filter(|t| t.step > 0)
                        .collect::<Vec<_>>(),
                ));
            if let Some(sensitivity_summary) = sensitivity_summary {
                lines.push(format!("Safety : {sensitivity_summary}"));
            }
            lines.push(format!("Config : {}", self.config_source()));
            lines.push(format!("Report : {}", artifacts.log_path.display()));
            if let Some(trace_path) = &artifacts.trace_path {
                lines.push(format!("Trace  : {}", trace_path.display()));
            }
        }
        lines.push(format!("Summary: {}", Self::final_summary_preview(summary)));
        lines
    }

    fn render_path_sensitivity_summary_line(
        stats: &[SessionTracePathSensitivityStat<'static>],
    ) -> Option<String> {
        if stats.is_empty() {
            return None;
        }

        let summary = stats
            .iter()
            .take(3)
            .map(|stat| format!("{}={}", stat.category, stat.calls))
            .collect::<Vec<_>>()
            .join(", ");
        if stats.len() > 3 {
            Some(format!("{summary} (+{} more)", stats.len() - 3))
        } else {
            Some(summary)
        }
    }

    fn trace_turn_sensitivity(turn: &Turn) -> Option<&'static str> {
        const PREFIX: &str = "[sensitivity: ";
        let start = turn.output.find(PREFIX)? + PREFIX.len();
        let rest = &turn.output[start..];
        let end = rest.find(']')?;
        let category = &rest[..end];

        match category {
            "outside_workspace" => Some("outside_workspace"),
            "repo_meta" => Some("repo_meta"),
            "project_config" => Some("project_config"),
            "runtime_state" => Some("runtime_state"),
            "prompts" => Some("prompts"),
            "knowledge" => Some("knowledge"),
            "memory" => Some("memory"),
            "source" => Some("source"),
            _ => None,
        }
    }

    fn stop_reason_label(&self, stop_reason: StopReason) -> &'static str {
        match stop_reason {
            StopReason::Success => "success",
            StopReason::MaxSteps => "max_steps",
            StopReason::NoProgress => "no_progress",
            StopReason::Error => "error",
        }
    }

    fn trace_preview(text: &str, max: usize) -> String {
        if text.trim().is_empty() {
            return "(empty)".to_string();
        }
        let redacted = redaction::redact(text);
        let single_line = redacted.replace('\n', "\\n").replace('\r', "");
        let trimmed = single_line.trim();
        let mut chars = trimmed.chars();
        let collected: String = chars.by_ref().take(max).collect();
        if chars.next().is_some() {
            format!("{collected}...")
        } else if collected.is_empty() {
            "(empty)".to_string()
        } else {
            collected
        }
    }

    pub(crate) fn resume_effective_task(&self, requested_task: &str) -> String {
        if requested_task.trim().eq_ignore_ascii_case("continue") {
            if let Some(goal) = self.task_state().goal() {
                return format!("Continue the interrupted task: {goal}");
            }
        }
        requested_task.to_string()
    }

    pub(crate) fn resume_guidance(&self) -> Option<String> {
        if !self.resumed_from_task_state() {
            return None;
        }

        let mut lines = Vec::new();
        if let Some(goal) = self.task_state().goal() {
            lines.push(format!("- Restored goal from persisted task state: {goal}"));
        }
        if let Some(next) = self.task_state().next_best_action_hint() {
            lines.push(format!("- Last known next best action: {next}"));
        }
        if let Some(safety) = self.recent_resume_safety_summary() {
            lines.push(format!(
                "- Recent path-sensitive writes from the previous session: {safety}. Verify those areas before broad follow-up changes."
            ));
        }
        if self.task_state().has_recent_stall_pressure() {
            lines.push("- The saved state shows exploration-heavy churn without recent implementation or verification progress; resume with a concrete implementation, verification, clarification, or blocker-reporting step.".to_string());
        } else if self.task_state().strategy_change_required() {
            lines.push("- The saved state already required a strategy change; do not resume with another exploration-only step.".to_string());
        }

        if lines.is_empty() {
            None
        } else {
            Some(lines.join("\n"))
        }
    }

    fn recent_resume_safety_summary(&self) -> Option<String> {
        self.history()
            .turns
            .iter()
            .rev()
            .find(|turn| turn.tool == "load_session" && turn.success)
            .and_then(|turn| {
                turn.output.lines().find_map(|line| {
                    line.trim()
                        .strip_prefix("**Safety:** ")
                        .map(str::trim)
                        .filter(|summary| !summary.is_empty())
                        .map(ToOwned::to_owned)
                })
            })
    }
}

#[cfg(test)]
mod tests {
    use super::{SessionTracePathSensitivityStat, SweAgent, TaskStatePersistenceAction};
    use crate::agent::core::StopReason;
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
        assert!(trace.contains("\"schema_version\": 3"));
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

        assert_eq!(trace["schema_version"], 3);
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
        // Regression test for S-048: a sensitive token present in a turn's output
        // must not appear verbatim in the trace event detail field.
        // trace_events() builds detail via trace_preview(), which calls redact().
        let repo = tempfile::TempDir::new().unwrap();
        let repo_path = repo.path().to_str().unwrap();
        let mut agent = test_agent(repo_path);
        agent.session_init();

        agent.history_mut().push(Turn {
            step: 1,
            thought: "Reading config".to_string(),
            tool: "read_file".to_string(),
            args: serde_json::json!({ "path": "config.toml" }),
            // Output contains a raw secret token — simulates a file read that
            // returned a line with an API key.
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
        // Regression: sensitive token in the thought field must also be redacted.
        let repo = tempfile::TempDir::new().unwrap();
        let repo_path = repo.path().to_str().unwrap();
        let mut agent = test_agent(repo_path);
        agent.session_init();

        agent.history_mut().push(Turn {
            step: 1,
            // Thought echoes back what looked like a secret from context.
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
        // S-053 regression: recent_signatures and recent_progress_markers from
        // the previous session must not carry stall/loop signal into the new one.
        let repo = tempfile::TempDir::new().unwrap();
        let mut agent = test_agent(repo.path().to_str().unwrap());

        // Build a TaskState that would trigger strategy_change_required() —
        // four consecutive exploration signatures in recent_signatures and
        // four "exploration" markers in recent_progress_markers.
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

        // Cross-session context is preserved.
        assert_eq!(agent.task_state().goal(), Some("fix the auth bug"));
        assert_eq!(
            agent.task_state().next_best_action_hint(),
            Some("Run focused verification before broader edits.")
        );

        // Session-local signal fields are cleared.
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
}
