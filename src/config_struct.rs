use serde::{Deserialize, Serialize};
use std::path::Path;

// Re-export so callers can use BackendKind without importing shell directly.
pub use crate::shell::BackendKind;

// Path constants
pub const AI_DIR: &str = ".ai";
pub const STATE_DIR: &str = "state";
pub const KNOWLEDGE_DIR: &str = "knowledge";
pub const PROMPTS_DIR: &str = "prompts";
pub const LOGS_DIR: &str = "logs";

/// Per-role model overrides.
/// Any field left as None falls back to AgentConfig::model.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModelRouter {
    /// Exploring, analyzing, planning — before any edits
    pub thinking: Option<String>,
    /// Writing or editing files (write_file, str_replace)
    pub coding: Option<String>,
    /// Searching the codebase (search_in_files, find_files, list_dir, read_file)
    pub search: Option<String>,
    /// Running commands (run_command)
    pub execution: Option<String>,
    /// Reading images (read_image) — requires a vision-capable model
    pub vision: Option<String>,
}

impl ModelRouter {
    pub fn resolve(&self, role: &ModelRole, default: &str) -> String {
        let override_ = match role {
            ModelRole::Thinking => self.thinking.as_deref(),
            ModelRole::Coding => self.coding.as_deref(),
            ModelRole::Search => self.search.as_deref(),
            ModelRole::Execution => self.execution.as_deref(),
            ModelRole::Vision => self.vision.as_deref(),
        };
        override_.unwrap_or(default).to_string()
    }
}

/// Which kind of work the agent is doing on this step.
#[derive(Debug, Clone)]
pub enum ModelRole {
    Thinking,
    Coding,
    Search,
    Execution,
    Vision,
}

impl ModelRole {
    /// Derive role from the tool the LLM chose to call.
    pub fn from_tool(tool: &str) -> Self {
        match tool {
            "write_file" | "str_replace" => Self::Coding,
            "search_in_files" | "find_files" | "list_dir" | "read_file" => Self::Search,
            "run_command" => Self::Execution,
            "read_image" => Self::Vision,
            _ => Self::Thinking,
        }
    }

    pub fn label(&self) -> &str {
        match self {
            Self::Thinking => "thinking",
            Self::Coding => "coding",
            Self::Search => "search",
            Self::Execution => "execution",
            Self::Vision => "vision",
        }
    }
}

// ─── Browser configuration ────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BrowserConfig {
    /// AWP server URL, e.g. "http://127.0.0.1:9222".
    /// Start with: plasmate serve --protocol awp --host 127.0.0.1 --port 9222
    #[serde(default)]
    pub awp_url: Option<String>,
    #[serde(default)]
    pub screenshot_dir: Option<String>,
}

impl BrowserConfig {
    pub fn is_configured(&self) -> bool {
        self.awp_url.is_some()
    }

    pub fn effective_screenshot_dir(&self, repo_root: &Path) -> std::path::PathBuf {
        match &self.screenshot_dir {
            Some(d) => std::path::PathBuf::from(d),
            None => repo_root.join(".ai/screenshots"),
        }
    }
}

// ─── Built-in prompts (compiled into the binary) ──────────────────────────────

pub const PROMPT_DEFAULT: &str = include_str!("prompts/default.md");
const PROMPT_BOSS: &str = include_str!("prompts/boss.md");
const PROMPT_RESEARCH: &str = include_str!("prompts/research.md");
const PROMPT_DEVELOPER: &str = include_str!("prompts/developer.md");
const PROMPT_NAVIGATOR: &str = include_str!("prompts/navigator.md");
const PROMPT_QA: &str = include_str!("prompts/qa.md");
const PROMPT_REVIEWER: &str = include_str!("prompts/reviewer.md");
const PROMPT_MEMORY: &str = include_str!("prompts/memory.md");

pub fn builtin_role_prompts() -> [(&'static str, &'static str); 8] {
    [
        ("default.md", PROMPT_DEFAULT),
        ("boss.md", PROMPT_BOSS),
        ("research.md", PROMPT_RESEARCH),
        ("developer.md", PROMPT_DEVELOPER),
        ("navigator.md", PROMPT_NAVIGATOR),
        ("qa.md", PROMPT_QA),
        ("reviewer.md", PROMPT_REVIEWER),
        ("memory.md", PROMPT_MEMORY),
    ]
}

// ─── Agent roles ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    #[default]
    Default,
    Boss,
    Research,
    Developer,
    Navigator,
    Qa,
    Reviewer,
    Memory,
}

impl Role {
    pub fn role_from_str(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "default" => Some(Self::Default),
            "boss" => Some(Self::Boss),
            "research" => Some(Self::Research),
            "developer" | "dev" => Some(Self::Developer),
            "navigator" | "nav" => Some(Self::Navigator),
            "qa" => Some(Self::Qa),
            "reviewer" | "review" => Some(Self::Reviewer),
            "memory" => Some(Self::Memory),
            _ => None,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Boss => "boss",
            Self::Research => "research",
            Self::Developer => "developer",
            Self::Navigator => "navigator",
            Self::Qa => "qa",
            Self::Reviewer => "reviewer",
            Self::Memory => "memory",
        }
    }

    /// Tool allowlist for this role. Empty vec = all tools allowed (Default role).
    /// For named roles, returns core tools only (no optional groups).
    pub fn allowed_tools(&self) -> Vec<&'static str> {
        self.allowed_tools_with_groups(&[])
    }

    /// Tool allowlist with optional capability groups applied.
    pub fn allowed_tools_with_groups(&self, tool_groups: &[String]) -> Vec<&'static str> {
        match self {
            Self::Default => vec![], // unrestricted
            _ => crate::tools::allowed_tools_for_role_with_groups(self.name(), tool_groups),
        }
    }

    /// Load system prompt: first try .ai/prompts/<role>.md, then built-in.
    /// Tool catalog is injected with core tools only (no optional groups).
    pub fn system_prompt(&self, repo_root: &Path) -> String {
        self.system_prompt_with_groups(repo_root, &[])
    }

    /// Load system prompt with optional capability groups applied to the tool catalog.
    pub fn system_prompt_with_groups(&self, repo_root: &Path, tool_groups: &[String]) -> String {
        let prompt_path = repo_root
            .join(".ai/prompts")
            .join(format!("{}.md", self.name()));
        if let Ok(text) = std::fs::read_to_string(&prompt_path) {
            if !text.trim().is_empty() {
                tracing::info!(
                    "Role '{}': loaded prompt from {}",
                    self.name(),
                    prompt_path.display()
                );
                return crate::tools::inject_tool_catalog_with_groups(
                    &text,
                    Some(self.name()),
                    tool_groups,
                );
            }
        }
        crate::tools::inject_tool_catalog_with_groups(
            self.builtin_prompt(),
            Some(self.name()),
            tool_groups,
        )
    }

    fn builtin_prompt(&self) -> &'static str {
        match self {
            Self::Default => PROMPT_DEFAULT,
            Self::Boss => PROMPT_BOSS,
            Self::Research => PROMPT_RESEARCH,
            Self::Developer => PROMPT_DEVELOPER,
            Self::Navigator => PROMPT_NAVIGATOR,
            Self::Qa => PROMPT_QA,
            Self::Reviewer => PROMPT_REVIEWER,
            Self::Memory => PROMPT_MEMORY,
        }
    }
}

// ─── AgentConfig ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    /// LLM service base URL.
    /// Examples:
    ///   "http://localhost:11434"       — Ollama (default)
    ///   "https://api.openai.com"       — OpenAI
    ///   "https://api.anthropic.com"    — Anthropic
    ///   "https://api.minimax.io"       — MiniMax (OpenAI or Anthropic compatible)
    pub llm_url: String,

    /// Wire protocol to use. Defaults to "ollama".
    /// Accepted values: "ollama", "openai", "anthropic"
    #[serde(default)]
    pub llm_backend: BackendKind,

    /// API key for the LLM service (not needed for local Ollama).
    /// Can also be set via the LLM_API_KEY environment variable.
    #[serde(default)]
    pub llm_api_key: Option<String>,

    /// Default model — used when a role has no override in [models]
    pub model: String,
    /// Per-role model overrides (all optional)
    #[serde(default)]
    pub models: ModelRouter,
    /// Sampling temperature
    pub temperature: f64,
    /// Max tokens per LLM response
    pub max_tokens: u32,
    /// Keep last N steps in full in context; older ones are summarized
    pub history_window: usize,
    /// Truncate tool output to this many chars before sending to LLM
    pub max_output_chars: usize,
    /// Max depth for nested spawn_agent calls (prevents runaway recursion).
    /// depth=0 is the top-level agent; sub-agents increment this counter.
    /// When depth >= max_depth, spawn_agent and spawn_agents are refused.
    #[serde(default = "default_max_depth")]
    pub max_depth: usize,
    /// System prompt (default role only — roles override via Role::system_prompt)
    pub system_prompt: String,
    /// Telegram bot token for ask_human / notifications (optional)
    #[serde(default)]
    pub telegram_token: Option<String>,
    /// Telegram chat_id to send messages to (optional)
    #[serde(default)]
    pub telegram_chat_id: Option<String>,
    /// Optional headless browser backend 
    #[serde(default)]
    pub browser: BrowserConfig,
    /// Log level: "error", "warn", "info", "debug", "trace"
    #[serde(default = "default_log_level")]
    pub log_level: String,
    /// Log format: "text", "json"
    #[serde(default = "default_log_format")]
    pub log_format: String,
    /// Allowed programs for run_command (empty = all allowed)
    #[serde(default)]
    pub command_allowlist: Vec<String>,
    /// Optional capability groups to enable beyond core tools.
    /// Possible values: "browser", "background", "github"
    /// Example: tool_groups = ["browser", "github"]
    #[serde(default)]
    pub tool_groups: Vec<String>,
}

fn default_max_depth() -> usize {
    3
}
fn default_log_level() -> String {
    "info".to_string()
}
fn default_log_format() -> String {
    "text".to_string()
}

impl AgentConfig {
    /// Resolve the API key: config field first, then LLM_API_KEY env var.
    pub fn effective_api_key(&self) -> Option<String> {
        self.llm_api_key
            .clone()
            .filter(|s| !s.trim().is_empty())
            .or_else(|| std::env::var("LLM_API_KEY").ok().filter(|s| !s.is_empty()))
    }

    /// Build a [`BackendConfig`] for the default model.
    pub fn backend_config(&self) -> crate::shell::BackendConfig {
        crate::shell::BackendConfig {
            url: self.llm_url.clone(),
            kind: self.llm_backend.clone(),
            api_key: self.effective_api_key(),
            model: self.model.clone(),
            temperature: self.temperature,
            max_tokens: self.max_tokens,
        }
    }

    /// Sensible step budget for a sub-agent when the caller did not specify max_steps.
    pub fn max_steps_for_sub_agent(&self) -> usize {
        15
    }

    /// Role-aware step budget. Navigator and researcher need more steps on large codebases.
    pub fn max_steps_for_role(&self, role: &str) -> usize {
        match role {
            "navigator" => 20,
            "research" => 20,
            "developer" => 20,
            "qa" => 15,
            "reviewer" => 12,
            "memory" => 8,
            _ => self.max_steps_for_sub_agent(),
        }
    }
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            llm_url: "http://localhost:11434".to_string(),
            llm_backend: BackendKind::Ollama,
            llm_api_key: None,
            model: "qwen3.5:cloud".to_string(),
            models: ModelRouter::default(),
            temperature: 0.0,
            max_tokens: 4096,
            history_window: 8,
            max_output_chars: 6000,
            max_depth: 3,
            system_prompt: PROMPT_DEFAULT.to_string(),
            telegram_token: None,
            telegram_chat_id: None,
            browser: BrowserConfig::default(),
            log_level: default_log_level(),
            log_format: default_log_format(),
            command_allowlist: vec![],
            tool_groups: vec![],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Role;
    use crate::tools::{canonical_tool_name, extract_tool_names_from_prompt, find_tool_spec};
    use serial_test::serial;

    use super::AgentConfig;

    #[test]
    fn every_role_allowlist_entry_exists_in_tool_registry() {
        let roles = [
            Role::Boss,
            Role::Research,
            Role::Developer,
            Role::Navigator,
            Role::Qa,
            Role::Reviewer,
            Role::Memory,
        ];
        for role in roles {
            for tool in role.allowed_tools() {
                assert!(
                    find_tool_spec(tool).is_some(),
                    "role '{}' references unknown tool '{}'",
                    role.name(),
                    tool
                );
            }
        }
    }

    #[test]
    fn every_prompt_advertised_tool_exists_in_tool_registry() {
        let roles = [
            Role::Default,
            Role::Boss,
            Role::Research,
            Role::Developer,
            Role::Navigator,
            Role::Qa,
            Role::Reviewer,
            Role::Memory,
        ];
        for role in roles {
            for tool in extract_tool_names_from_prompt(role.builtin_prompt()) {
                assert!(
                    find_tool_spec(&tool).is_some(),
                    "role '{}' prompt advertises unknown tool '{}'",
                    role.name(),
                    tool
                );
            }
        }
    }

    #[test]
    fn non_default_role_prompts_only_advertise_allowed_tools() {
        // All optional groups enabled — prompts may advertise browser/background/gitHub tools
        // as long as the role is in that tool's allowed_roles.
        let all_groups: Vec<String> = vec![
            "browser".to_string(),
            "background".to_string(),
            "github".to_string(),
        ];

        let roles = [
            Role::Boss,
            Role::Research,
            Role::Developer,
            Role::Navigator,
            Role::Qa,
            Role::Reviewer,
            Role::Memory,
        ];
        for role in roles {
            let allowed_canonical: std::collections::HashSet<&'static str> = role
                .allowed_tools_with_groups(&all_groups)
                .iter()
                .filter_map(|tool| canonical_tool_name(tool))
                .collect();

            for tool in extract_tool_names_from_prompt(role.builtin_prompt()) {
                let prompt_canonical = canonical_tool_name(&tool).unwrap_or_else(|| {
                    panic!("prompt tool '{}' should resolve through registry", tool)
                });
                assert!(
                    allowed_canonical.contains(prompt_canonical),
                    "role '{}' prompt advertises tool '{}' (canonical '{}') not in its allowlist (even with all groups enabled)",
                    role.name(),
                    tool,
                    prompt_canonical
                );
            }
        }
    }

    #[test]
    #[serial]
    fn effective_api_key_prefers_config_value_over_env() {
        let previous = std::env::var("LLM_API_KEY").ok();
        std::env::set_var("LLM_API_KEY", "env-key");

        let cfg = AgentConfig {
            llm_api_key: Some("config-key".to_string()),
            ..AgentConfig::default()
        };

        let effective = cfg.effective_api_key();

        match previous {
            Some(value) => std::env::set_var("LLM_API_KEY", value),
            None => std::env::remove_var("LLM_API_KEY"),
        }

        assert_eq!(effective.as_deref(), Some("config-key"));
    }

    #[test]
    #[serial]
    fn effective_api_key_falls_back_to_env_when_config_is_absent() {
        let previous = std::env::var("LLM_API_KEY").ok();
        std::env::set_var("LLM_API_KEY", "env-key");

        let cfg = AgentConfig {
            llm_api_key: None,
            ..AgentConfig::default()
        };

        let effective = cfg.effective_api_key();

        match previous {
            Some(value) => std::env::set_var("LLM_API_KEY", value),
            None => std::env::remove_var("LLM_API_KEY"),
        }

        assert_eq!(effective.as_deref(), Some("env-key"));
    }
}
