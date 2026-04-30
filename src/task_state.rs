use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::history::Turn;
use crate::loop_policy;
use crate::text::first_line;

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

    /// Format working memory for injection into the LLM prompt.
    ///
    /// Priority order (most important first, so weak models read it top-to-bottom):
    ///   1. Goal
    ///   2. Current priority (next best action) — highlighted if set
    ///   3. Active blocker — highlighted if any
    ///   4. Recent meaningful progress
    ///   5. Attempted actions and artifacts
    ///   6. Loop/stall pressure signals (only when non-trivial)
    pub fn format_for_prompt(&self) -> String {
        let mut out = String::new();

        // ── 1. Goal ───────────────────────────────────────────────────────────
        out.push_str(&format!(
            "Goal: {}\n",
            if self.goal.is_empty() {
                "(not set yet)"
            } else {
                &self.goal
            }
        ));

        // ── 2. Current priority ───────────────────────────────────────────────
        let next = self
            .next_best_action
            .as_deref()
            .filter(|s| !s.trim().is_empty());
        if let Some(action) = next {
            out.push_str(&format!(">>> PRIORITY: {action}\n"));
        } else {
            out.push_str("Priority: (not decided yet)\n");
        }

        // ── 3. Active blocker ─────────────────────────────────────────────────
        if !self.blocked_on.is_empty() {
            let latest = self.blocked_on.last().map(String::as_str).unwrap_or("");
            out.push_str(&format!(">>> BLOCKED: {latest}\n"));
            if self.blocked_on.len() > 1 {
                let earlier = &self.blocked_on[..self.blocked_on.len() - 1];
                out.push_str(&format!(
                    "Earlier blockers: {}\n",
                    earlier.join(" | ")
                ));
            }
        } else {
            out.push_str("Blocked on: (none)\n");
        }

        // ── 4. Recent meaningful progress ─────────────────────────────────────
        out.push_str(&format!(
            "Recent progress: {}\n",
            self.recent_meaningful_progress_summary()
        ));

        // ── 5. History summary ────────────────────────────────────────────────
        out.push_str(&format!(
            "Attempted actions: {}\n",
            join_or_none(&self.attempted_actions)
        ));
        out.push_str(&format!(
            "Artifacts found: {}\n",
            join_or_none(&self.artifacts_found)
        ));

        // ── 6. Loop/stall pressure — only emit when active ────────────────────
        if !self.repeated_signatures.is_empty() {
            out.push_str(&format!(
                "Repeated tool signatures: {}\n",
                join_or_none(&self.repeated_signatures)
            ));
        }
        if self.has_recent_stall_pressure() {
            out.push_str("Stall pressure: YES — switch to implementation, verification, or finish\n");
        }
        if self.strategy_change_required() {
            out.push_str(&format!(
                "Strategy change required: YES — exploration budget exhausted ({}+ consecutive steps)\n",
                loop_policy::EXPLORATION_BUDGET
            ));
        } else if let Some(warning) = self.exploration_warning() {
            out.push_str(&format!("Exploration pressure: {warning}\n"));
        }

        out
    }

    pub fn goal(&self) -> Option<&str> {
        let goal = self.goal.trim();
        if goal.is_empty() { None } else { Some(goal) }
    }

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
        "read_file" | "list_dir" | "find_files" | "search_in_files" | "tree" | "outline"
        | "get_symbols" | "open_file_region" => Some(
            "Use the gathered context to take a narrower implementation, verification, or diff-inspection step."
                .to_string(),
        ),
        "write_file" | "str_replace" | "str_replace_multi" | "str_replace_fuzzy" => Some(
            "Verify the change with a focused read, diff, or test before making broader edits."
                .to_string(),
        ),
        "run_command" | "diff_repo" | "git_status" | "git_log" => Some(
            "Interpret the result and either proceed with the next code change or finish with a concrete summary."
                .to_string(),
        ),
        "checkpoint" => Some(
            "Progress recorded. Continue with the next planned step or call finish if done."
                .to_string(),
        ),
        "notify" => Some(
            "Notification sent. Continue with the next planned step."
                .to_string(),
        ),
        "ask_human" => Some(
            "Use the human's response to decide the next action or unblock progress."
                .to_string(),
        ),
        "browser_navigate" | "browser_get_text" | "browser_action" => Some(
            "Process the page content or SOM result, then take the next browser action or finish with findings."
                .to_string(),
        ),
        "spawn_agent" | "spawn_agents" => Some(
            "Read the memory_key to verify sub-agent results, then proceed to the next delegation or finish."
                .to_string(),
        ),
        _ => None,
    }
}

/// Classify a completed turn into a progress category for loop detection.
///
/// "implementation" and "verification" are the only meaningful progress markers.
/// checkpoint and notify count as "active" progress (not exploration, not blocked)
/// so that agents actively communicating their progress don't trip stall detection.
fn progress_marker(turn: &Turn) -> &'static str {
    if turn.success {
        match turn.tool.as_str() {
            "write_file" | "str_replace" | "str_replace_multi" | "str_replace_fuzzy" => {
                "implementation"
            }
            "run_command" | "diff_repo" | "git_status" | "git_log" => "verification",
            // checkpoint and notify are active steps — not exploration, not blocked.
            // Counting them as "active" prevents false stall pressure when an agent
            // is doing real work but every step is a coordination/comms action.
            "checkpoint" | "notify" => "active",
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

    #[test]
    fn format_for_prompt_shows_priority_before_history() {
        let mut state = TaskState::new();
        state.set_goal("Fix the parser");
        state.update_from_turn(&make_turn(1, "write_file", true));

        let prompt = state.format_for_prompt();
        let priority_pos = prompt.find(">>> PRIORITY").expect("PRIORITY marker must be present");
        let actions_pos = prompt.find("Attempted actions").expect("Attempted actions must be present");
        assert!(priority_pos < actions_pos, "PRIORITY must appear before Attempted actions");
    }

    #[test]
    fn format_for_prompt_shows_blocker_prominently() {
        let mut state = TaskState::new();
        state.set_goal("Deploy service");
        let blocked_turn = Turn {
            step: 1,
            thought: "try browser".to_string(),
            tool: "browser_get_text".to_string(),
            args: serde_json::json!({ "url": "https://example.com" }),
            output: "ERR browser connection is not configured".to_string(),
            success: false,
        };
        state.update_from_turn(&blocked_turn);

        let prompt = state.format_for_prompt();
        let blocker_pos = prompt.find(">>> BLOCKED").expect("BLOCKED marker must be present");
        let actions_pos = prompt.find("Attempted actions").expect("Attempted actions must be present");
        assert!(blocker_pos < actions_pos, "BLOCKED must appear before Attempted actions");
        assert!(prompt.contains("not configured"));
    }

    #[test]
    fn format_for_prompt_suppresses_stall_noise_when_clean() {
        let mut state = TaskState::new();
        state.set_goal("Write a function");
        state.update_from_turn(&make_turn(1, "write_file", true));

        let prompt = state.format_for_prompt();
        assert!(!prompt.contains("Stall pressure: YES"));
        assert!(!prompt.contains("Strategy change required: YES"));
        assert!(!prompt.contains("Repeated tool signatures"));
    }

    /// Regression: checkpoint and notify must not count as exploration steps.
    /// An agent calling checkpoint/notify multiple times must not trip stall detection.
    #[test]
    fn checkpoint_and_notify_do_not_trigger_exploration_pressure() {
        let mut state = TaskState::new();
        for step in 1..=4 {
            state.update_from_turn(&Turn {
                step,
                thought: "recording progress".to_string(),
                tool: if step % 2 == 0 { "checkpoint" } else { "notify" }.to_string(),
                args: serde_json::json!({ "note": "progress" }),
                output: "ok".to_string(),
                success: true,
            });
        }
        assert!(
            !state.has_exploration_pressure(),
            "checkpoint/notify must not trigger exploration pressure"
        );
        assert!(
            !state.has_recent_stall_pressure(),
            "checkpoint/notify must not trigger stall pressure"
        );
    }

    /// Regression: checkpoint gives a useful next_best_action hint.
    #[test]
    fn checkpoint_produces_next_best_action_hint() {
        let mut state = TaskState::new();
        state.update_from_turn(&Turn {
            step: 1,
            thought: "recording".to_string(),
            tool: "checkpoint".to_string(),
            args: serde_json::json!({ "note": "step done" }),
            output: "Checkpoint recorded".to_string(),
            success: true,
        });
        assert!(
            state.next_best_action_hint().is_some(),
            "checkpoint must set a next_best_action hint"
        );
    }
}
