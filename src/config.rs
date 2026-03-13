use serde::{Deserialize, Serialize};
use std::path::Path;

/// Per-role model overrides.
/// Any field left as None falls back to AgentConfig::model.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModelRouter {
    /// Exploring, analysing, planning — before any edits
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
            ModelRole::Thinking  => self.thinking.as_deref(),
            ModelRole::Coding    => self.coding.as_deref(),
            ModelRole::Search    => self.search.as_deref(),
            ModelRole::Execution => self.execution.as_deref(),
            ModelRole::Vision    => self.vision.as_deref(),
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
            "write_file" | "str_replace"                                => Self::Coding,
            "search_in_files" | "find_files" | "list_dir" | "read_file" => Self::Search,
            "run_command"                                               => Self::Execution,
            "read_image"                                                => Self::Vision,
            _                                                           => Self::Thinking,
        }
    }

    pub fn label(&self) -> &str {
        match self {
            Self::Thinking  => "thinking",
            Self::Coding    => "coding",
            Self::Search    => "search",
            Self::Execution => "execution",
            Self::Vision    => "vision",
        }
    }
}

// ─── Built-in prompts (compiled into the binary) ──────────────────────────────
//
// Each role has a corresponding src/prompts/<role>.md file.
// To override a prompt for a specific project, place a file at
// .ai/prompts/<role>.md in the repository root.

const PROMPT_DEFAULT:   &str = include_str!("prompts/default.md");
const PROMPT_BOSS:      &str = include_str!("prompts/boss.md");
const PROMPT_RESEARCH:  &str = include_str!("prompts/research.md");
const PROMPT_DEVELOPER: &str = include_str!("prompts/developer.md");
const PROMPT_NAVIGATOR: &str = include_str!("prompts/navigator.md");
const PROMPT_QA:        &str = include_str!("prompts/qa.md");
const PROMPT_REVIEWER:  &str = include_str!("prompts/reviewer.md");
const PROMPT_MEMORY:    &str = include_str!("prompts/memory.md");

// ─── Agent roles ──────────────────────────────────────────────────────────────

/// Predefined agent roles — each has a fixed tool allowlist and a built-in
/// system prompt compiled from src/prompts/<role>.md.
/// The prompt can be overridden at runtime by placing a file at
/// .ai/prompts/<role>.md in the repository root.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// Full tool access — used when no role is specified
    #[default]
    Default,
    /// Orchestrator: breaks down tasks, delegates, tracks progress
    Boss,
    /// Researcher: web search and documentation reading
    Research,
    /// Developer: reads and edits code, runs commands
    Developer,
    /// Navigator: explores and understands codebase structure
    Navigator,
    /// Quality assurance: runs tests, checks diffs, reports issues
    Qa,
    /// Reviewer: static code review — reads code only, never executes
    Reviewer,
    /// Memory manager: reads and writes .ai/ state
    Memory,
}

impl Role {
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "default"              => Some(Self::Default),
            "boss"                 => Some(Self::Boss),
            "research"             => Some(Self::Research),
            "developer" | "dev"    => Some(Self::Developer),
            "navigator" | "nav"    => Some(Self::Navigator),
            "qa"                   => Some(Self::Qa),
            "reviewer" | "review"  => Some(Self::Reviewer),
            "memory"               => Some(Self::Memory),
            _                      => None,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::Default   => "default",
            Self::Boss      => "boss",
            Self::Research  => "research",
            Self::Developer => "developer",
            Self::Navigator => "navigator",
            Self::Qa        => "qa",
            Self::Reviewer  => "reviewer",
            Self::Memory    => "memory",
        }
    }

    /// Tool allowlist for this role. Empty slice = all tools allowed.
    pub fn allowed_tools(&self) -> &'static [&'static str] {
        match self {
            Self::Default => &[],  // unrestricted

            Self::Boss => &[
                "memory_read", "memory_write",
                "tree", "ask_human", "web_search",
                "spawn_agent", "notify",
                "finish",
            ],

            Self::Research => &[
                "web_search", "fetch_url",
                "memory_read", "memory_write",
                "ask_human", "finish",
            ],

            Self::Developer => &[
                "read_file", "write_file", "str_replace",
                "run_command", "diff_repo",
                "git_status", "git_commit", "git_log", "git_stash",
                "get_symbols", "outline", "get_signature", "find_references",
                "memory_read", "memory_write",
                "github_api", "test_coverage", "notify",
                "finish",
            ],

            Self::Navigator => &[
                "tree", "list_dir", "find_files",
                "search_in_files", "find_references",
                "read_file", "get_symbols", "outline",
                "finish",
            ],

            Self::Qa => &[
                "run_command", "read_file",
                "search_in_files", "diff_repo",
                "git_status", "git_log",
                "memory_read", "memory_write",
                "ask_human", "github_api", "test_coverage", "notify",
                "finish",
            ],

            // Reviewer: read-only. No write_file, str_replace, run_command,
            // git_commit, git_stash, spawn_agent, notify, test_coverage, github_api.
            Self::Reviewer => &[
                "read_file", "search_in_files", "find_references",
                "get_symbols", "outline", "get_signature",
                "diff_repo", "git_log",
                "memory_read", "memory_write",
                "ask_human",
                "finish",
            ],

            Self::Memory => &[
                "memory_read", "memory_write", "finish",
            ],
        }
    }

    /// Load system prompt: first try .ai/prompts/<role>.md, then built-in.
    pub fn system_prompt(&self, repo_root: &Path) -> String {
        let prompt_path = repo_root
            .join(".ai/prompts")
            .join(format!("{}.md", self.name()));
        if let Ok(text) = std::fs::read_to_string(&prompt_path) {
            if !text.trim().is_empty() {
                tracing::info!("Role '{}': loaded prompt from {}", self.name(), prompt_path.display());
                return text;
            }
        }
        self.builtin_prompt().to_string()
    }

    fn builtin_prompt(&self) -> &'static str {
        match self {
            Self::Default   => PROMPT_DEFAULT,
            Self::Boss      => PROMPT_BOSS,
            Self::Research  => PROMPT_RESEARCH,
            Self::Developer => PROMPT_DEVELOPER,
            Self::Navigator => PROMPT_NAVIGATOR,
            Self::Qa        => PROMPT_QA,
            Self::Reviewer  => PROMPT_REVIEWER,
            Self::Memory    => PROMPT_MEMORY,
        }
    }
}

// ─── AgentConfig ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    /// Ollama base URL
    pub ollama_base_url: String,
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
    /// System prompt (default role only — roles override this via Role::system_prompt)
    pub system_prompt: String,
    /// Telegram bot token for ask_human / notifications (optional)
    #[serde(default)]
    pub telegram_token: Option<String>,
    /// Telegram chat_id to send messages to (optional)
    #[serde(default)]
    pub telegram_chat_id: Option<String>,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            ollama_base_url: "http://localhost:11434".to_string(),
            model: "qwen3.5:9b".to_string(),
            models: ModelRouter::default(),
            temperature: 0.0,
            max_tokens: 4096,
            history_window: 8,
            max_output_chars: 6000,
            system_prompt: PROMPT_DEFAULT.to_string(),
            telegram_token: None,
            telegram_chat_id: None,
        }
    }
}

impl AgentConfig {
    /// Load config following the priority chain:
    ///   --config <path>       — explicit CLI override
    ///   ./config.toml         — local project config
    ///   ~/.do_it/config.toml  — global user config
    ///   built-in defaults
    pub fn load(explicit: Option<&str>) -> Self {
        if let Some(path) = explicit {
            return Self::load_file(path).unwrap_or_else(|| {
                tracing::warn!("Config '{}' not found or invalid, using defaults", path);
                Self::default()
            });
        }
        if Path::new("config.toml").exists() {
            if let Some(cfg) = Self::load_file("config.toml") {
                tracing::info!("Config: ./config.toml");
                return cfg;
            }
        }
        if let Some(global) = global_config_path() {
            if global.exists() {
                if let Some(cfg) = Self::load_file(&global.to_string_lossy()) {
                    tracing::info!("Config: {}", global.display());
                    return cfg;
                }
            }
        }
        tracing::info!("Config: built-in defaults");
        Self::default()
    }

    /// Legacy alias — kept for compatibility.
    pub fn load_or_default(path: &str) -> Self {
        Self::load_file(path).unwrap_or_else(|| {
            tracing::warn!("Config '{}' not found, using defaults", path);
            Self::default()
        })
    }

    fn load_file(path: &str) -> Option<Self> {
        if !Path::new(path).exists() {
            return None;
        }
        match std::fs::read_to_string(path).map(|s| toml::from_str::<Self>(&s)) {
            Ok(Ok(mut cfg)) => {
                // Apply ~/.do_it/system_prompt.md if config still has the default prompt
                if cfg.system_prompt == PROMPT_DEFAULT {
                    if let Some(sp) = load_global_system_prompt() {
                        cfg.system_prompt = sp;
                    }
                }
                Some(cfg)
            }
            Ok(Err(e)) => { tracing::error!("Config parse error in '{path}': {e}"); None }
            Err(e)     => { tracing::error!("Config read error '{}': {e}", path); None }
        }
    }
}

// ─── Global config helpers ────────────────────────────────────────────────────

/// Returns ~/.do_it/ as a PathBuf, or None if home dir cannot be determined.
pub fn global_config_dir() -> Option<std::path::PathBuf> {
    home_dir().map(|h| h.join(".do_it"))
}

/// Returns ~/.do_it/config.toml
pub fn global_config_path() -> Option<std::path::PathBuf> {
    global_config_dir().map(|d| d.join("config.toml"))
}

/// Returns ~/.do_it/user_profile.md
pub fn global_user_profile_path() -> Option<std::path::PathBuf> {
    global_config_dir().map(|d| d.join("user_profile.md"))
}

/// Returns ~/.do_it/boss_notes.md
pub fn global_boss_notes_path() -> Option<std::path::PathBuf> {
    global_config_dir().map(|d| d.join("boss_notes.md"))
}

/// Load ~/.do_it/system_prompt.md if it exists and is non-empty.
pub fn load_global_system_prompt() -> Option<String> {
    let path = global_config_dir()?.join("system_prompt.md");
    std::fs::read_to_string(&path).ok().filter(|s| !s.trim().is_empty())
}

/// Cross-platform home directory.
fn home_dir() -> Option<std::path::PathBuf> {
    #[cfg(windows)]
    {
        std::env::var("USERPROFILE").ok()
            .or_else(|| {
                let drive = std::env::var("HOMEDRIVE").ok()?;
                let path  = std::env::var("HOMEPATH").ok()?;
                Some(format!("{drive}{path}"))
            })
            .map(std::path::PathBuf::from)
    }
    #[cfg(not(windows))]
    {
        std::env::var("HOME").ok().map(std::path::PathBuf::from)
    }
}

/// Ensure ~/.do_it/ exists with default files on first run.
pub fn ensure_global_config() {
    let dir = match global_config_dir() {
        Some(d) => d,
        None => {
            tracing::warn!("Cannot determine home directory — skipping ~/.do_it init");
            return;
        }
    };
    if dir.exists() {
        return;
    }
    if let Err(e) = std::fs::create_dir_all(&dir) {
        tracing::warn!("Cannot create {}: {e}", dir.display());
        return;
    }

    let files: &[(&str, &str)] = &[
        ("config.toml", r#"# do_it — global configuration
# Used when no local ./config.toml is found.
# Local config.toml always takes precedence.

ollama_base_url  = "http://localhost:11434"
model            = "qwen3.5:9b"
temperature      = 0.0
max_tokens       = 4096
history_window   = 8
max_output_chars = 6000

[models]
# thinking  = "qwen3.5:9b"
# coding    = "qwen3-coder-next"
# search    = "qwen3.5:4b"
# execution = "qwen3.5:4b"
# vision    = "qwen3.5:9b"

# telegram_token   = "1234567890:ABCdef..."
# telegram_chat_id = "123456789"
"#),
        ("system_prompt.md", r#"# Global system prompt override for do_it
#
# Delete these comments and write your own prompt to activate this file.
# When active, this file overrides the built-in default prompt.
# Role-specific prompts (boss, developer, etc.) are NOT affected by this file.
"#),
        ("user_profile.md", r#"# User profile
#
# The Boss agent reads this file at the start of every session.
# Describe your preferences so the agent works the way you like.
#
# Suggested sections:
#
# ## Communication
# - Preferred language: English
# - Response style: concise, technical
#
# ## Development stack
# - Primary language: Rust
# - Preferred crates: tokio, serde, sqlx, anyhow
# - Architecture notes: isolated Cargo workspaces for mixed wasm32/native targets
#
# ## Workflow preferences
# - Prefer full rewrites over patch accumulation when fixes cause regressions
# - Always run clippy before committing
"#),
        ("boss_notes.md", r#"# Boss notes
#
# Cross-project insights accumulated by the Boss agent.
# The Boss appends here when it discovers something worth keeping beyond the current project.
#
"#),
    ];

    let mut created = Vec::new();
    for (name, content) in files {
        let path = dir.join(name);
        if std::fs::write(&path, content).is_ok() {
            created.push(path);
        }
    }

    println!("╔══════════════════════════════════════════╗");
    println!("║   First run — initialized ~/.do_it/      ║");
    println!("╚══════════════════════════════════════════╝");
    for path in &created {
        println!("  Created: {}", path.display());
    }
    println!("  Edit these files to set your global defaults.");
    println!();
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_roles_parse() {
        let cases = [
            ("boss",      Role::Boss),
            ("research",  Role::Research),
            ("developer", Role::Developer),
            ("dev",       Role::Developer),
            ("navigator", Role::Navigator),
            ("nav",       Role::Navigator),
            ("qa",        Role::Qa),
            ("reviewer",  Role::Reviewer),
            ("review",    Role::Reviewer),
            ("memory",    Role::Memory),
            ("default",   Role::Default),
        ];
        for (s, expected) in cases {
            assert_eq!(Role::from_str(s), Some(expected), "failed for '{s}'");
        }
    }

    #[test]
    fn unknown_role_returns_none() {
        assert_eq!(Role::from_str("wizard"), None);
        assert_eq!(Role::from_str(""), None);
    }

    #[test]
    fn reviewer_cannot_mutate() {
        let allowed = Role::Reviewer.allowed_tools();
        let forbidden = [
            "write_file", "str_replace", "run_command",
            "git_commit", "git_stash", "github_api",
            "spawn_agent", "notify", "test_coverage",
        ];
        for tool in forbidden {
            assert!(
                !allowed.contains(&tool),
                "reviewer allowlist must not contain '{tool}'"
            );
        }
    }

    #[test]
    fn reviewer_has_read_tools() {
        let allowed = Role::Reviewer.allowed_tools();
        let required = [
            "read_file", "search_in_files", "find_references",
            "outline", "diff_repo", "git_log",
            "memory_read", "memory_write", "ask_human", "finish",
        ];
        for tool in required {
            assert!(
                allowed.contains(&tool),
                "reviewer allowlist must contain '{tool}'"
            );
        }
    }

    #[test]
    fn default_role_is_unrestricted() {
        assert!(Role::Default.allowed_tools().is_empty());
    }

    #[test]
    fn all_roles_have_non_empty_builtin_prompt() {
        let roles = [
            Role::Default, Role::Boss, Role::Research, Role::Developer,
            Role::Navigator, Role::Qa, Role::Reviewer, Role::Memory,
        ];
        for role in &roles {
            assert!(
                !role.builtin_prompt().trim().is_empty(),
                "builtin prompt for '{}' is empty", role.name()
            );
        }
    }

    #[test]
    fn global_memory_paths_are_under_dot_do_it() {
        // Only meaningful when home dir is available
        if let Some(profile) = global_user_profile_path() {
            assert!(profile.to_string_lossy().contains(".do_it"));
            assert!(profile.to_string_lossy().ends_with("user_profile.md"));
        }
        if let Some(notes) = global_boss_notes_path() {
            assert!(notes.to_string_lossy().contains(".do_it"));
            assert!(notes.to_string_lossy().ends_with("boss_notes.md"));
        }
    }
}
