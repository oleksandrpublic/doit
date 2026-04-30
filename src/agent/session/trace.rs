use crate::agent::core::{StopReason, SweAgent};
use crate::config_struct::{AI_DIR, STATE_DIR};
use crate::history::Turn;
use crate::tools;
use serde::Serialize;

// ---------------------------------------------------------------------------
// Trace types
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub(crate) struct SessionTrace<'a> {
    pub schema_version: u32,
    pub session_nr: u64,
    pub role: &'a str,
    pub config_source: &'a str,
    pub task: &'a str,
    pub stop_reason: &'a str,
    pub started_at: &'a str,
    pub ended_at: String,
    pub max_steps: usize,
    pub steps_used: usize,
    pub resumed_from_task_state: bool,
    pub summary_preview: String,
    pub total_calls: usize,
    pub ok_calls: usize,
    pub err_calls: usize,
    pub tool_stats: Vec<SessionTraceToolStat<'a>>,
    pub path_sensitivity_stats: Vec<SessionTracePathSensitivityStat<'static>>,
    pub events: Vec<SessionTraceEvent<'a>>,
}

#[derive(Debug, Serialize)]
pub(crate) struct SessionTraceToolStat<'a> {
    pub tool: &'a str,
    pub calls: usize,
    pub err_calls: usize,
}

#[derive(Debug, Serialize)]
pub(crate) struct SessionTracePathSensitivityStat<'a> {
    pub category: &'a str,
    pub calls: usize,
}

#[derive(Debug, Serialize)]
pub(crate) struct SessionTraceEvent<'a> {
    pub event: &'static str,
    pub step: Option<usize>,
    pub tool: Option<&'a str>,
    pub success: Option<bool>,
    /// Serialized tool arguments (first 120 chars). Present for "turn" events;
    /// empty string for "session_started" and "session_finished".
    /// Added in schema_version 4 to support step-level replay.
    pub args_preview: String,
    pub detail: String,
}

// ---------------------------------------------------------------------------
// impl SweAgent
// ---------------------------------------------------------------------------

impl SweAgent {
    pub(crate) fn write_session_trace(
        &self,
        path: std::path::PathBuf,
        task: &str,
        final_summary: &str,
        stop_reason: StopReason,
        steps_used: usize,
        started_at_str: &str,
        total_calls: usize,
        ok_calls: usize,
        err_calls: usize,
        turns: &[&Turn],
    ) -> Option<std::path::PathBuf> {
        use crate::agent::session::render::SweAgentRender;
        let path_sensitivity_stats = Self::trace_path_sensitivity_stats(turns);
        let trace = SessionTrace {
            schema_version: 4,
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
            summary_preview: Self::trace_preview(
                &SweAgentRender::final_summary_preview(final_summary),
                220,
            ),
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

    pub(crate) fn trace_events<'a>(
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
            args_preview: String::new(),
            detail: Self::trace_preview(task, 160),
        });

        for turn in turns {
            let sensitivity_note = Self::trace_turn_sensitivity(turn)
                .map(|sensitivity| format!(" sensitivity={sensitivity}"))
                .unwrap_or_default();
            let args_preview = Self::trace_preview(&turn.args.to_string(), 120);
            events.push(SessionTraceEvent {
                event: "turn",
                step: Some(turn.step),
                tool: Some(turn.tool.as_str()),
                success: Some(turn.success),
                args_preview,
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
            args_preview: String::new(),
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

    pub(crate) fn trace_tool_stats<'a>(turns: &[&'a Turn]) -> Vec<SessionTraceToolStat<'a>> {
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

    pub(crate) fn trace_path_sensitivity_stats(
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

    pub(crate) fn trace_turn_sensitivity(turn: &Turn) -> Option<&'static str> {
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

    pub(crate) fn stop_reason_label(&self, stop_reason: StopReason) -> &'static str {
        match stop_reason {
            StopReason::Success => "success",
            StopReason::MaxSteps => "max_steps",
            StopReason::NoProgress => "no_progress",
            StopReason::Error => "error",
        }
    }

    pub(crate) fn trace_preview(text: &str, max: usize) -> String {
        if text.trim().is_empty() {
            return "(empty)".to_string();
        }
        let redacted = crate::redaction::redact(text);
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

    /// Append a decision annotation to `.ai/state/session_decisions.md`.
    pub(crate) fn append_decision(&self, step: usize, tool: &str, decision: &str) {
        let decision = decision.trim();
        if decision.is_empty() {
            return;
        }
        let state_dir = self.root().join(AI_DIR).join(STATE_DIR);
        let _ = std::fs::create_dir_all(&state_dir);
        let path = state_dir.join("session_decisions.md");
        let now = tools::chrono_now();
        let role = self.role().name();
        let entry = format!(
            "\n## Step {step} - {now} [{role}]\nTool: {tool}\nDecision: {decision}\n"
        );
        let existing = std::fs::read_to_string(&path).unwrap_or_default();
        let _ = std::fs::write(&path, format!("{existing}{entry}"));
    }
}
