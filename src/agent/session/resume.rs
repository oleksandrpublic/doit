use crate::agent::core::SweAgent;

/// Session recovery hierarchy (read-only reference — do not reorder without updating DOCS.md):
///
/// 1. `task_state.json` — structured working memory (goal, actions, artifacts, blockers).
///    Restored by `restore_task_state_from_disk()` in `session_init`. Sets `resumed_from_task_state = true`.
///
/// 2. `.ai/state/last_session.md` — narrative note written at the end of every session.
///    Injected into history as step 0 (tool = "load_session") by `session_init`.
///    Read by `recent_resume_safety_summary()` to extract the **Safety:** line.
///
/// 3. Plan files — checked by `find_stale_plan_file()` only when task_state was restored.
///    Canonical location: `.ai/state/current_plan.md` (written via `memory_write("plan")`).
///    Legacy / ad-hoc locations also checked: `.ai/plan.md`, `PLAN.md`, `plan.md`.
///
/// Sources 1–3 do not conflict: each covers a different aspect of state.
/// `memory_read("plan")` and `find_stale_plan_file` refer to the same canonical file;
/// there is no double-injection risk.
impl SweAgent {
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
        if let Some(plan_path) = self.find_stale_plan_file() {
            lines.push(format!(
                "- A plan file exists at `{plan_path}` from a previous session. \
                 Verify it still reflects the current goal before following it blindly."
            ));
        }

        if lines.is_empty() {
            None
        } else {
            Some(lines.join("\n"))
        }
    }

    /// Return the relative path of a plan file left over from a previous session, if any.
    ///
    /// Checks the canonical location first (`.ai/state/current_plan.md`, written by
    /// `memory_write("plan")`), then legacy ad-hoc locations that agents may have created
    /// outside the standard hierarchy.
    pub(crate) fn find_stale_plan_file(&self) -> Option<String> {
        if !self.resumed_from_task_state() {
            return None;
        }
        let candidates = [
            ".ai/state/current_plan.md", // canonical: memory_write("plan")
            ".ai/plan.md",               // legacy ad-hoc
            "PLAN.md",                   // legacy ad-hoc
            "plan.md",                   // legacy ad-hoc
        ];
        for rel in &candidates {
            if self.root().join(rel).is_file() {
                return Some((*rel).to_string());
            }
        }
        None
    }

    pub(crate) fn recent_resume_safety_summary(&self) -> Option<String> {
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
