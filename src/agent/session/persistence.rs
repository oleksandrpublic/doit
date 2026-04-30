use crate::agent::core::{StopReason, SweAgent};
use crate::config_struct::{AI_DIR, LOGS_DIR, STATE_DIR};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TaskStatePersistenceAction {
    Save,
    Clear,
}

impl SweAgent {
    pub(crate) fn task_state_path(&self) -> PathBuf {
        self.root()
            .join(AI_DIR)
            .join(STATE_DIR)
            .join("task_state.json")
    }

    pub(crate) fn session_trace_path(&self) -> PathBuf {
        self.root()
            .join(AI_DIR)
            .join(LOGS_DIR)
            .join(format!("session-{:03}.trace.json", self.session_nr()))
    }

    pub(crate) fn save_task_state(&self) {
        let path = self.task_state_path();
        let _ = std::fs::write(&path, self.task_state().to_json_pretty());
    }

    pub(crate) fn task_state_persistence_action(
        &self,
        stop_reason: StopReason,
    ) -> TaskStatePersistenceAction {
        if self.depth() > 0 || stop_reason.is_success() {
            TaskStatePersistenceAction::Clear
        } else {
            TaskStatePersistenceAction::Save
        }
    }

    pub(crate) fn apply_task_state_persistence(&self, stop_reason: StopReason) {
        match self.task_state_persistence_action(stop_reason) {
            TaskStatePersistenceAction::Save => self.save_task_state(),
            TaskStatePersistenceAction::Clear => self.clear_task_state(),
        }
    }

    pub(crate) fn clear_task_state(&self) {
        let path = self.task_state_path();
        let _ = std::fs::remove_file(&path);
    }

    pub(crate) fn restore_task_state_from_disk(&mut self) -> bool {
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
        self.history_mut().push(crate::history::Turn {
            step: 0,
            thought: "Loading persisted task state".to_string(),
            tool: "load_task_state".to_string(),
            args: serde_json::json!({ "path": path.display().to_string() }),
            output: state.format_for_prompt(),
            success: true,
        });
        true
    }
}
