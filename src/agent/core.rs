use anyhow::Result;
use std::path::PathBuf;

use crate::config_struct::{AgentConfig, ModelRouter, Role};
use crate::history::History;
use crate::shell::LlmClient;
use crate::task_state::TaskState;
use crate::tools::TelegramConfig;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    Success,
    MaxSteps,
    NoProgress,
    Error,
}

impl StopReason {
    pub fn is_success(self) -> bool {
        matches!(self, Self::Success)
    }
}

pub struct SweAgent {
    llm: LlmClient,
    default_model: String,
    router: ModelRouter,
    root: PathBuf,
    history: History,
    task_state: TaskState,
    max_steps: usize,
    max_output_chars: usize,
    system_prompt: String,
    tg: TelegramConfig,
    role: Role,
    /// Current nesting depth (0 = top-level, incremented for each spawn_agent call)
    depth: usize,
    /// Session number set by session_init(), used in session_finish()
    session_nr: u64,
    /// Stored so sub-agents can inherit connection settings
    cfg_snapshot: AgentConfig,
    /// Track consecutive parse failures to break cycles
    consecutive_parse_failures: usize,
    /// Whether task_state was restored from a persisted session snapshot
    resumed_from_task_state: bool,
    /// Original task source path if the task came from a file
    task_source: Option<String>,
    /// Human-readable provenance of the resolved config used for this run
    config_source: String,
    /// TUI handle — Some at depth=0 when stdout is a TTY, None for sub-agents
    tui: Option<crate::tui::TuiHandle>,
    /// Cached global files read once at session start — avoids per-step disk I/O.
    cached_boss_notes: String,
    cached_user_profile: String,
}

impl SweAgent {
    pub fn new(cfg: AgentConfig, repo: &str, max_steps: usize, role: Role) -> Result<Self> {
        Self::new_with_depth(cfg, repo, max_steps, role, 0)
    }

    /// Create a sub-agent at a specific nesting depth.
    /// Called from spawn_agent / spawn_agents in tools.rs.
    pub fn new_with_depth(
        cfg: AgentConfig,
        repo: &str,
        max_steps: usize,
        role: Role,
        depth: usize,
    ) -> Result<Self> {
        let raw = PathBuf::from(repo)
            .canonicalize()
            .map_err(|e| anyhow::anyhow!("Repo path '{}' not accessible: {e}", repo))?;

        // canonicalize() on Windows produces verbatim UNC paths like \\?\D:\...
        // Strip the prefix so paths stay human-readable and tool-compatible.
        let root = {
            let s = raw.to_string_lossy();
            if let Some(stripped) = s.strip_prefix(r"\\?\") {
                PathBuf::from(stripped)
            } else {
                raw
            }
        };

        let llm = LlmClient::new(cfg.backend_config());

        let system_prompt = if role != Role::Default {
            role.system_prompt_with_groups(&root, &cfg.tool_groups)
        } else {
            crate::tools::inject_tool_catalog_with_groups(
                &cfg.system_prompt,
                None,
                &cfg.tool_groups,
            )
        };

        Ok(Self {
            llm,
            default_model: cfg.model.clone(),
            router: cfg.models.clone(),
            root,
            history: History::new(cfg.history_window),
            task_state: TaskState::new(),
            max_steps,
            max_output_chars: cfg.max_output_chars,
            system_prompt,
            tg: TelegramConfig {
                token: cfg.telegram_token.clone(),
                chat_id: cfg.telegram_chat_id.clone(),
            },
            role,
            depth,
            session_nr: 0,
            cfg_snapshot: cfg,
            consecutive_parse_failures: 0,
            resumed_from_task_state: false,
            task_source: None,
            config_source: "built-in defaults".to_string(),
            tui: None,
            cached_boss_notes: String::new(),
            cached_user_profile: String::new(),
        })
    }

    pub(crate) fn all_models(&self) -> Vec<&str> {
        let mut seen = vec![self.default_model.as_str()];
        for opt in [
            self.router.thinking.as_deref(),
            self.router.coding.as_deref(),
            self.router.search.as_deref(),
            self.router.execution.as_deref(),
            self.router.vision.as_deref(),
        ]
        .into_iter()
        .flatten()
        {
            if !seen.contains(&opt) {
                seen.push(opt);
            }
        }
        seen
    }

    pub(crate) fn root(&self) -> &PathBuf {
        &self.root
    }

    pub(crate) fn depth(&self) -> usize {
        self.depth
    }

    pub(crate) fn role(&self) -> Role {
        self.role.clone()
    }

    pub(crate) fn history(&self) -> &History {
        &self.history
    }

    pub(crate) fn task_state(&self) -> &TaskState {
        &self.task_state
    }

    pub(crate) fn max_steps(&self) -> usize {
        self.max_steps
    }

    pub(crate) fn max_output_chars(&self) -> usize {
        self.max_output_chars
    }

    pub(crate) fn session_nr(&self) -> u64 {
        self.session_nr
    }

    pub(crate) fn llm(&self) -> &LlmClient {
        &self.llm
    }

    pub(crate) fn default_model(&self) -> &str {
        &self.default_model
    }

    pub(crate) fn router(&self) -> &ModelRouter {
        &self.router
    }

    pub(crate) fn system_prompt(&self) -> &str {
        &self.system_prompt
    }

    pub(crate) fn tg(&self) -> &TelegramConfig {
        &self.tg
    }

    pub(crate) fn consecutive_parse_failures(&self) -> usize {
        self.consecutive_parse_failures
    }

    pub(crate) fn set_consecutive_parse_failures(&mut self, n: usize) {
        self.consecutive_parse_failures = n;
    }

    pub(crate) fn inc_consecutive_parse_failures(&mut self) {
        self.consecutive_parse_failures += 1;
    }

    pub(crate) fn set_session_nr(&mut self, n: u64) {
        self.session_nr = n;
    }

    pub fn history_mut(&mut self) -> &mut History {
        &mut self.history
    }

    pub(crate) fn task_state_mut(&mut self) -> &mut TaskState {
        &mut self.task_state
    }

    pub(crate) fn cfg_snapshot(&self) -> &AgentConfig {
        &self.cfg_snapshot
    }

    pub(crate) fn resumed_from_task_state(&self) -> bool {
        self.resumed_from_task_state
    }

    pub(crate) fn set_resumed_from_task_state(&mut self, resumed: bool) {
        self.resumed_from_task_state = resumed;
    }

    pub(crate) fn task_source(&self) -> Option<&str> {
        self.task_source.as_deref()
    }

    pub(crate) fn set_task_source(&mut self, source: Option<String>) {
        self.task_source = source;
    }

    pub(crate) fn config_source(&self) -> &str {
        &self.config_source
    }

    pub(crate) fn set_config_source(&mut self, source: String) {
        self.config_source = source;
    }

    pub(crate) fn tui(&self) -> Option<&crate::tui::TuiHandle> {
        self.tui.as_ref()
    }

    pub(crate) fn take_tui(&mut self) -> Option<crate::tui::TuiHandle> {
        self.tui.take()
    }

    pub(crate) fn set_tui(&mut self, handle: Option<crate::tui::TuiHandle>) {
        self.tui = handle;
    }

    /// Send a TUI event if the handle exists, otherwise no-op.
    pub(crate) fn cached_boss_notes(&self) -> &str {
        &self.cached_boss_notes
    }

    pub(crate) fn cached_user_profile(&self) -> &str {
        &self.cached_user_profile
    }

    pub(crate) fn set_cached_boss_notes(&mut self, s: String) {
        self.cached_boss_notes = s;
    }

    pub(crate) fn set_cached_user_profile(&mut self, s: String) {
        self.cached_user_profile = s;
    }

    pub(crate) fn tui_send(&self, ev: crate::tui::TuiEvent) {
        if let Some(h) = &self.tui {
            h.send(ev);
        }
    }
}

pub enum StepOutcome {
    Continue,
    Finished {
        summary: String,
        stop_reason: StopReason,
    },
}
