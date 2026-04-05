use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::history::Turn;
use crate::loop_policy;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TaskState {
    goal: String,
    attempted_actions: Vec<String>,
    artifacts_found: Vec<String>,
    blocked_on: Vec<String>,
    repeated_signatures: Vec<String>,
    recent_signatures: Vec<String>,
    #[serde(default)]
    recent_progress_markers: Vec<String>,
    next_best_action: Option<String>,
}

pub use crate::loop_policy::EXPLORATION_TOOLS;

impl TaskState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_goal(&mut self, goal: impl Into<String>) {
        let goal = goal.into().trim().to_string();
        if !goal.is_empty() {
            self.goal = goal;
        }
    }

    pub fn update_from_turn(&mut self, turn: &Turn) {
        if turn.step == 0 {
            return;
        }

        push_limited(
            &mut self.attempted_actions,
            format!(
                "step {}: {} {}",
                turn.step,
                turn.tool,
                summarize_args(&turn.args)
            ),
            8,
        );

        if let Some(artifact) = infer_artifact(turn) {
            push_limited(&mut self.artifacts_found, artifact, 8);
        }

        if let Some(blocker) = infer_blocker(turn) {
            push_limited(&mut self.blocked_on, blocker, 6);
        }

        let signature = normalized_signature(turn);
        let repeat_count = self
            .recent_signatures
            .iter()
            .filter(|existing| *existing == &signature)
            .count()
            + 1;
        if repeat_count >= 2 {
            push_limited(
                &mut self.repeated_signatures,
                format!("{signature} x{repeat_count}"),
                6,
            );
        }
        push_recent(&mut self.recent_signatures, signature, 12);
        push_recent(
            &mut self.recent_progress_markers,
            progress_marker(turn).to_string(),
            12,
        );

        self.next_best_action = infer_next_best_action(turn);
    }

    pub fn format_for_prompt(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "Goal: {}\n",
            if self.goal.is_empty() {
                "(not set yet)"
            } else {
                &self.goal
            }
        ));
        out.push_str(&format!(
            "Attempted actions: {}\n",
            join_or_none(&self.attempted_actions)
        ));
        out.push_str(&format!(
            "Artifacts found: {}\n",
            join_or_none(&self.artifacts_found)
        ));
        out.push_str(&format!("Blocked on: {}\n", join_or_none(&self.blocked_on)));
        out.push_str(&format!(
            "Repeated tool signatures: {}\n",
            join_or_none(&self.repeated_signatures)
        ));
        out.push_str(&format!(
            "Recent meaningful progress: {}\n",
            self.recent_meaningful_progress_summary()
        ));
        out.push_str(&format!(
            "Recent stall pressure: {}\n",
            if self.has_recent_stall_pressure() {
                "yes: at least 3 of the last 4 steps were exploration-heavy without meaningful implementation or verification progress"
            } else {
                "no"
            }
        ));
        out.push_str(&format!(
            "Exploration pressure: {}\n",
            self.exploration_warning().unwrap_or("(none)")
        ));
        out.push_str(&format!(
            "Exploration budget: {} consecutive exploration step(s) remaining before strategy change is required (triggers at 4+)\n",
            self.exploration_budget_remaining()
        ));
        out.push_str(&format!(
            "Strategy change required: {}\n",
            if self.strategy_change_required() {
                "yes"
            } else {
                "no"
            }
        ));
        out.push_str(&format!(
            "Next best action: {}\n",
            self.next_best_action
                .as_deref()
                .filter(|s| !s.trim().is_empty())
                .unwrap_or("(not decided yet)")
        ));
        out
    }

    pub fn goal(&self) -> Option<&str> {
        let goal = self.goal.trim();
        if goal.is_empty() { None } else { Some(goal) }
    }

    /// Clear session-local signal fields that reflect within-session tool call
    /// history.  Call this after restoring TaskState from disk so that the
    /// resumed session starts from a clean slate for loop/stall detection,
    /// while preserving cross-session context (goal, blockers, artifacts, etc.).
    pub fn clear_session_signals(&mut self) {
        self.recent_signatures.clear();
        self.recent_progress_markers.clear();
    }

    pub fn next_best_action_hint(&self) -> Option<&str> {
        self.next_best_action
            .as_deref()
            .filter(|s| !s.trim().is_empty())
    }

    pub fn to_json_pretty(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".to_string())
    }

    pub fn from_json(text: &str) -> Option<Self> {
        serde_json::from_str(text).ok()
    }

    pub fn is_resume_worthy(&self) -> bool {
        self.goal().is_some()
            || self.next_best_action_hint().is_some()
            || has_non_empty_entries(&self.attempted_actions)
            || has_non_empty_entries(&self.artifacts_found)
            || has_non_empty_entries(&self.blocked_on)
            || has_non_empty_entries(&self.repeated_signatures)
            || has_non_empty_entries(&self.recent_signatures)
            || has_non_empty_entries(&self.recent_progress_markers)
    }

    pub fn exploration_warning(&self) -> Option<&str> {
        loop_policy::exploration_warning(
            self.consecutive_exploration_steps(),
            self.has_recent_stall_pressure(),
            self.has_repeated_exploration_pressure(),
        )
    }

    pub fn has_exploration_pressure(&self) -> bool {
        self.exploration_warning().is_some()
    }

    pub fn has_repeated_exploration_pressure(&self) -> bool {
        self.consecutive_exploration_steps() >= loop_policy::EXPLORATION_BUDGET
            && !self.repeated_signatures.is_empty()
    }

    pub fn has_recent_stall_pressure(&self) -> bool {
        loop_policy::recent_stall_pressure(&self.recent_signatures, &self.recent_progress_markers)
    }

    pub fn exploration_budget_remaining(&self) -> usize {
        loop_policy::exploration_budget_remaining(self.consecutive_exploration_steps())
    }

    pub fn strategy_change_required(&self) -> bool {
        loop_policy::strategy_change_required(
            self.consecutive_exploration_steps(),
            self.has_recent_stall_pressure(),
        )
    }

    fn consecutive_exploration_steps(&self) -> usize {
        self.recent_signatures
            .iter()
            .rev()
            .take_while(|sig| is_exploration_action(sig))
            .count()
    }

    fn recent_meaningful_progress_summary(&self) -> String {
        let mut labels = Vec::new();

        for marker in self
            .recent_progress_markers
            .iter()
            .rev()
            .take(loop_policy::RECENT_PROGRESS_WINDOW)
        {
            let Some(label) = progress_label(marker) else {
                continue;
            };
            if !labels.iter().any(|existing| existing == label) {
                labels.push(label.to_string());
            }
        }

        if labels.is_empty() {
            format!(
                "(none in last {} steps)",
                loop_policy::RECENT_PROGRESS_WINDOW
            )
        } else {
            labels.join(" | ")
        }
    }
}

fn push_limited(bucket: &mut Vec<String>, item: String, max_len: usize) {
    let item = item.trim().to_string();
    if item.is_empty() {
        return;
    }
    if bucket
        .last()
        .map(|existing| existing == &item)
        .unwrap_or(false)
    {
        return;
    }
    if bucket.contains(&item) {
        return;
    }
    bucket.push(item);
    if bucket.len() > max_len {
        bucket.remove(0);
    }
}

fn push_recent(bucket: &mut Vec<String>, item: String, max_len: usize) {
    let item = item.trim().to_string();
    if item.is_empty() {
        return;
    }
    bucket.push(item);
    if bucket.len() > max_len {
        bucket.remove(0);
    }
}

fn is_exploration_action(signature: &str) -> bool {
    let tool = signature
        .split_once(' ')
        .map(|(tool, _)| tool)
        .unwrap_or(signature);
    loop_policy::is_exploration_tool(tool)
}

fn summarize_args(args: &Value) -> String {
    if let Some(obj) = args.as_object() {
        let mut parts = Vec::new();
        for key in [
            "path", "dir", "url", "name", "selector", "program", "pattern",
        ] {
            if let Some(value) = obj.get(key) {
                let rendered = scalar_preview(value);
                if !rendered.is_empty() {
                    parts.push(format!("{key}={rendered}"));
                }
            }
        }
        if !parts.is_empty() {
            return format!("({})", parts.join(", "));
        }
    }
    String::new()
}

fn normalized_signature(turn: &Turn) -> String {
    let args = summarize_args(&turn.args);
    let output = first_line(&turn.output, 60);
    let args = if args.is_empty() {
        "()".to_string()
    } else {
        args
    };
    format!("{} {} -> {}", turn.tool, args, output)
}

fn infer_artifact(turn: &Turn) -> Option<String> {
    let obj = turn.args.as_object()?;
    match turn.tool.as_str() {
        "read_file" | "write_file" | "str_replace" | "outline" | "get_symbols"
        | "open_file_region" => obj
            .get("path")
            .map(|v| format!("file touched: {}", scalar_preview(v))),
        "list_dir" | "find_files" | "tree" | "search_in_files" | "find_references" => obj
            .get("dir")
            .or_else(|| obj.get("path"))
            .map(|v| format!("location inspected: {}", scalar_preview(v))),
        "diff_repo" => Some("repository diff inspected".to_string()),
        _ => None,
    }
}

fn infer_blocker(turn: &Turn) -> Option<String> {
    let output = turn.output.to_lowercase();
    if turn.success {
        return None;
    }

    if output.contains("not configured") {
        return Some(format!(
            "{} is unavailable because the environment is not configured",
            turn.tool
        ));
    }
    if output.contains("not implemented") || output.contains("limited tool") {
        return Some(format!(
            "{} is currently limited and may need a fallback",
            turn.tool
        ));
    }
    if output.contains("missing") || output.contains("required") {
        return Some(format!(
            "{} call was rejected because required arguments were missing",
            turn.tool
        ));
    }
    if output.contains("not allowed") {
        return Some(format!("{} is not allowed for the current role", turn.tool));
    }

    Some(format!(
        "{} failed: {}",
        turn.tool,
        first_line(&turn.output, 100)
    ))
}

fn infer_next_best_action(turn: &Turn) -> Option<String> {
    if !turn.success {
        return Some(
            "Choose a different real tool or ask_human if the task is blocked by configuration or missing capability."
                .to_string(),
        );
    }

    match turn.tool.as_str() {
        "read_file" | "list_dir" | "find_files" | "search_in_files" | "tree" | "outline" | "get_symbols" | "open_file_region" => {
            Some("Use the gathered context to take a narrower implementation, verification, or diff-inspection step.".to_string())
        }
        "write_file" | "str_replace" => {
            Some("Verify the change with a focused read, diff, or test before making broader edits.".to_string())
        }
        "run_command" | "diff_repo" | "git_status" | "git_log" => {
            Some("Interpret the result and either proceed with the next code change or finish with a concrete summary.".to_string())
        }
        _ => None,
    }
}

fn progress_marker(turn: &Turn) -> &'static str {
    if turn.success {
        match turn.tool.as_str() {
            "write_file" | "str_replace" => "implementation",
            "run_command" | "diff_repo" | "git_status" | "git_log" => "verification",
            _ if loop_policy::is_exploration_tool(turn.tool.as_str()) => "exploration",
            _ => "other",
        }
    } else if loop_policy::is_exploration_tool(turn.tool.as_str()) {
        "exploration"
    } else {
        "blocked"
    }
}

fn progress_label(marker: &str) -> Option<&'static str> {
    loop_policy::progress_label(marker)
}

fn scalar_preview(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        _ => String::new(),
    }
}

fn first_line(s: &str, max: usize) -> String {
    let line = s.lines().next().unwrap_or("").trim();
    let mut chars = line.chars();
    let collected: String = chars.by_ref().take(max).collect();
    if chars.next().is_some() {
        format!("{collected}…")
    } else {
        collected
    }
}

fn join_or_none(items: &[String]) -> String {
    if items.is_empty() {
        "(none)".to_string()
    } else {
        items.join(" | ")
    }
}

fn has_non_empty_entries(items: &[String]) -> bool {
    items.iter().any(|item| !item.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::Turn;

    fn make_turn(step: usize, tool: &str, success: bool) -> Turn {
        Turn {
            step,
            thought: "test".to_string(),
            tool: tool.to_string(),
            args: serde_json::json!({ "path": "src/lib.rs" }),
            output: "ok".to_string(),
            success,
        }
    }

    #[test]
    fn update_from_turn_ignores_step_zero() {
        // Turns with step == 0 are internal session housekeeping (load_session,
        // load_task_state). They must not contaminate task_state fields.
        // Guards against regressions that remove the step == 0 early-return.
        let mut state = TaskState::new();
        let turn = Turn {
            step: 0,
            thought: "Loading previous session".to_string(),
            tool: "load_session".to_string(),
            args: serde_json::json!({}),
            output: "some session summary".to_string(),
            success: true,
        };
        state.update_from_turn(&turn);

        assert!(state.attempted_actions.is_empty());
        assert!(state.artifacts_found.is_empty());
        assert!(state.blocked_on.is_empty());
        assert!(state.recent_signatures.is_empty());
        assert!(state.recent_progress_markers.is_empty());
        assert!(state.next_best_action.is_none());
    }

    #[test]
    fn is_resume_worthy_false_for_default_state() {
        // A freshly constructed or deserialized-empty TaskState must not be
        // considered resume-worthy. This is the gate in restore_task_state_from_disk.
        // Guards against regressions that make is_resume_worthy always return true.
        let state = TaskState::new();
        assert!(!state.is_resume_worthy());
    }

    #[test]
    fn is_resume_worthy_true_when_goal_is_set() {
        let mut state = TaskState::new();
        state.set_goal("Fix the auth bug");
        assert!(state.is_resume_worthy());
    }

    #[test]
    fn is_resume_worthy_true_when_only_next_best_action_is_set() {
        // next_best_action alone (without a goal) is enough to resume.
        let mut state = TaskState::new();
        state.update_from_turn(&make_turn(1, "read_file", true));
        // read_file sets next_best_action but may or may not set goal.
        // Force next_best_action via JSON round-trip to test the field directly.
        let json = r#"{"goal":"","attempted_actions":[],"artifacts_found":[],"blocked_on":[],"repeated_signatures":[],"recent_signatures":[],"recent_progress_markers":[],"next_best_action":"Run focused verification."}"#;
        let state = TaskState::from_json(json).unwrap();
        assert!(state.is_resume_worthy());
    }

    #[test]
    fn update_from_turn_records_attempted_action() {
        let mut state = TaskState::new();
        state.update_from_turn(&make_turn(3, "write_file", true));
        assert_eq!(state.attempted_actions.len(), 1);
        assert!(state.attempted_actions[0].contains("write_file"));
        assert!(state.attempted_actions[0].starts_with("step 3:"));
    }

    #[test]
    fn update_from_turn_does_not_duplicate_last_attempted_action() {
        // push_limited must drop an item that is identical to the current last entry.
        // Prevents noise in the prompt when the same action is repeated.
        let mut state = TaskState::new();
        let turn = make_turn(1, "write_file", true);
        state.update_from_turn(&turn);
        state.update_from_turn(&turn);
        assert_eq!(state.attempted_actions.len(), 1);
    }

    #[test]
    fn infer_blocker_categorises_not_configured_error() {
        let mut state = TaskState::new();
        let turn = Turn {
            step: 1,
            thought: "fetch page".to_string(),
            tool: "browser_get_text".to_string(),
            args: serde_json::json!({ "url": "https://example.com" }),
            output: "ERR browser connection is not configured".to_string(),
            success: false,
        };
        state.update_from_turn(&turn);
        assert_eq!(state.blocked_on.len(), 1);
        assert!(state.blocked_on[0].contains("not configured"));
    }

    #[test]
    fn clear_session_signals_resets_loop_detection_fields() {
        // Guards against regressions that remove clear_session_signals or
        // forget to call it on restore.
        let mut state = TaskState::new();
        for i in 1..=4 {
            state.update_from_turn(&make_turn(i, "list_dir", true));
        }
        assert!(state.has_exploration_pressure());

        state.clear_session_signals();

        assert!(!state.has_exploration_pressure());
        assert!(!state.has_recent_stall_pressure());
        assert!(!state.strategy_change_required());
    }
}
