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

/// Which kind of work the agent is doing on this step
#[derive(Debug, Clone)]
pub enum ModelRole {
    Thinking,
    Coding,
    Search,
    Execution,
    Vision,
}

impl ModelRole {
    /// Derive role from the tool the LLM chose to call
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


// ─── Agent roles ──────────────────────────────────────────────────────────────

/// Predefined agent roles — each has a fixed tool allowlist and a default
/// system prompt. The prompt can be overridden by placing a file at
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
    /// Memory manager: reads and writes .ai/ state
    Memory,
}

impl Role {
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "default"   => Some(Self::Default),
            "boss"      => Some(Self::Boss),
            "research"  => Some(Self::Research),
            "developer" | "dev" => Some(Self::Developer),
            "navigator" | "nav" => Some(Self::Navigator),
            "qa"        => Some(Self::Qa),
            "memory"    => Some(Self::Memory),
            _           => None,
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
            Self::Memory    => "memory",
        }
    }

    /// Tool allowlist for this role. Empty = all tools allowed.
    pub fn allowed_tools(&self) -> &'static [&'static str] {
        match self {
            Self::Default => &[],  // empty = unrestricted
            Self::Boss => &[
                "memory_read", "memory_write",
                "tree", "ask_human", "web_search",
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
                "get_symbols", "outline", "get_signature",
                "memory_read", "memory_write",
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
                "ask_human", "finish",
            ],
            Self::Memory => &[
                "memory_read", "memory_write", "finish",
            ],
        }
    }

    /// Load system prompt: first try .ai/prompts/<role>.md, then built-in.
    pub fn system_prompt(&self, repo_root: &std::path::Path) -> String {
        // Try file override first
        let prompt_path = repo_root
            .join(".ai/prompts")
            .join(format!("{}.md", self.name()));
        if let Ok(text) = std::fs::read_to_string(&prompt_path) {
            if !text.trim().is_empty() {
                tracing::info!("Role '{}': loaded prompt from {}", self.name(), prompt_path.display());
                return text;
            }
        }
        self.builtin_prompt()
    }

    fn builtin_prompt(&self) -> String {
        match self {
            Self::Default => DEFAULT_SYSTEM_PROMPT.to_string(),
            Self::Boss => BOSS_PROMPT.to_string(),
            Self::Research => RESEARCH_PROMPT.to_string(),
            Self::Developer => DEVELOPER_PROMPT.to_string(),
            Self::Navigator => NAVIGATOR_PROMPT.to_string(),
            Self::Qa => QA_PROMPT.to_string(),
            Self::Memory => MEMORY_PROMPT.to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    /// Ollama base URL
    pub ollama_base_url: String,

    /// Default model — used when a role has no override in models
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

    /// System prompt
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
            system_prompt: DEFAULT_SYSTEM_PROMPT.to_string(),
            telegram_token: None,
            telegram_chat_id: None,
        }
    }
}

impl AgentConfig {
    /// Load config following the priority chain:
    ///   --config <path>  (explicit, always used if provided and not the default sentinel)
    ///   ./config.toml    (local project config)
    ///   ~/.do_it/config.toml  (global user config)
    ///   built-in defaults
    ///
    /// `explicit` is Some when the user passed --config explicitly.
    pub fn load(explicit: Option<&str>) -> Self {
        // 1. Explicit --config path
        if let Some(path) = explicit {
            return Self::load_file(path).unwrap_or_else(|| {
                tracing::warn!("Config '{}' not found or invalid, using defaults", path);
                Self::default()
            });
        }

        // 2. Local ./config.toml
        if Path::new("config.toml").exists() {
            if let Some(cfg) = Self::load_file("config.toml") {
                tracing::info!("Config: ./config.toml");
                return cfg;
            }
        }

        // 3. ~/.do_it/config.toml
        if let Some(global) = global_config_path() {
            if global.exists() {
                if let Some(cfg) = Self::load_file(&global.to_string_lossy()) {
                    tracing::info!("Config: {}", global.display());
                    return cfg;
                }
            }
        }

        // 4. Built-in defaults
        tracing::info!("Config: built-in defaults");
        Self::default()
    }

    /// Legacy alias used internally — kept for compatibility.
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
                // If system_prompt is default, check for ~/.do_it/system_prompt.md override
                if cfg.system_prompt == DEFAULT_SYSTEM_PROMPT {
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

/// Returns ~/.do_it as a PathBuf, or None if home dir cannot be determined.
pub fn global_config_dir() -> Option<std::path::PathBuf> {
    home_dir().map(|h| h.join(".do_it"))
}

/// Returns ~/.do_it/config.toml
pub fn global_config_path() -> Option<std::path::PathBuf> {
    global_config_dir().map(|d| d.join("config.toml"))
}

/// Load ~/.do_it/system_prompt.md if it exists and is non-empty.
pub fn load_global_system_prompt() -> Option<String> {
    let path = global_config_dir()?.join("system_prompt.md");
    std::fs::read_to_string(&path).ok().filter(|s| !s.trim().is_empty())
}

/// Cross-platform home directory.
fn home_dir() -> Option<std::path::PathBuf> {
    // std::env::home_dir is deprecated but still works; use env vars as primary
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

/// Ensure ~/.do_it/ exists with default files.
/// Called once at startup. Prints a message if the directory was just created.
pub fn ensure_global_config() {
    let dir = match global_config_dir() {
        Some(d) => d,
        None => {
            tracing::warn!("Cannot determine home directory — skipping ~/.do_it init");
            return;
        }
    };

    if dir.exists() {
        return; // already initialised
    }

    // First run — create directory and default files
    if let Err(e) = std::fs::create_dir_all(&dir) {
        tracing::warn!("Cannot create {}: {e}", dir.display());
        return;
    }

    // ~/.do_it/config.toml
    let config_path = dir.join("config.toml");
    let default_config = r#"# do_it — global configuration
# This file is used when no local ./config.toml is found.
# Override any setting here; local config.toml always takes precedence.

ollama_base_url  = "http://localhost:11434"
model            = "qwen3.5:9b"
temperature      = 0.0
max_tokens       = 4096
history_window   = 8
max_output_chars = 6000

# Per-role model overrides (all optional — fall back to `model`)
[models]
# thinking  = "qwen3.5:9b"
# coding    = "qwen3-coder-next"
# search    = "qwen3.5:4b"
# execution = "qwen3.5:4b"
# vision    = "qwen3.5:9b"

# Telegram notifications for ask_human (optional)
# telegram_token   = "1234567890:ABCdef..."
# telegram_chat_id = "123456789"
"#;

    // ~/.do_it/system_prompt.md
    let prompt_path = dir.join("system_prompt.md");
    let default_prompt = r#"# Global system prompt for do_it
#
# This file is loaded when no role-specific prompt is active and
# no system_prompt is set in config.toml.
# Edit freely — it will not be overwritten on subsequent runs.
#
# The built-in default prompt is used if this file is left unchanged
# (i.e. still begins with a '#' comment block).
# Delete these comments and write your own prompt to activate it.
"#;

    let wrote_config = std::fs::write(&config_path, default_config).is_ok();
    let wrote_prompt = std::fs::write(&prompt_path, default_prompt).is_ok();

    println!("╔══════════════════════════════════════════╗");
    println!("║   First run — initialized ~/.do_it/      ║");
    println!("╚══════════════════════════════════════════╝");
    if wrote_config {
        println!("  Created: {}", config_path.display());
    }
    if wrote_prompt {
        println!("  Created: {}", prompt_path.display());
    }
    println!("  Edit these files to set your global defaults.");
    println!();
}

// ─── Default system prompt ────────────────────────────────────────────────────

const DEFAULT_SYSTEM_PROMPT: &str = r#"You are an autonomous software engineering agent running on a developer machine.
Your goal is to solve programming tasks by using a set of tools to interact with the filesystem, shell, internet, and your own persistent memory.

## Available tools

### Filesystem
- read_file(path, start_line?, end_line?)           — View a file with line numbers
- write_file(path, content)                          — Overwrite a file completely
- str_replace(path, old_str, new_str)               — Replace a unique string in a file
- list_dir(path?)                                    — List directory contents
- find_files(pattern, dir?)                          — Find files by name/glob
- search_in_files(pattern, dir?, ext?)              — Search text across files

### Execution
- run_command(program, args[], cwd?)                — Run an executable (no shell)
- diff_repo(base?, staged?, stat?)                  — Show git diff vs HEAD or any ref
- git_status(short?)                               — Working tree status + branch info
- git_commit(message, files?, allow_empty?)        — Stage files and commit
- git_log(n?, path?, oneline?)                     — Commit history
- git_stash(action, message?, index?)              — Stash management (push|pop|list|drop|show)

### Internet
- fetch_url(url, selector?)                          — Fetch a web page or docs
- web_search(query, max_results?)                   — Search the web via DuckDuckGo (no API key)
- tree(dir?, depth?, ignore?)                        — Recursive directory tree (ignores target/.git/etc by default)

### Code intelligence (regex-based, supports Rust/TS/JS/Python/C++/Kotlin)
- get_symbols(path, kinds?)                          — List all symbols (fn/struct/class/impl/enum/trait/type)
- outline(path)                                      — Structural outline with line numbers and signatures
- get_signature(path, name, lines?)                  — Full signature + doc comment for a named symbol
- find_references(name, dir?, ext?)                  — Find all usages of a symbol across the codebase

### Memory (.ai/ hierarchy)
- memory_read(key)                                   — Read a memory entry
- memory_write(key, content, append?)               — Write or append a memory entry

  Logical keys:
    "plan"            → .ai/state/current_plan.md
    "last_session"    → .ai/state/last_session.md
    "session_counter" → .ai/state/session_counter.txt
    "external"        → .ai/state/external_messages.md
    "history"         → .ai/logs/history.md
    "knowledge/<n>"   → .ai/knowledge/<n>.md
    "prompts/<n>"     → .ai/prompts/<n>.md
    any other key     → .ai/knowledge/<key>.md

### Human communication
- ask_human(question)                               — Ask the human via Telegram or console

### Session control
- finish(summary, success)                          — Signal completion

## Rules

1. At session start: read "last_session" and "plan" to restore context.
2. Explore before editing: use list_dir and read_file first.
3. Make minimal, targeted changes.
4. After editing, verify with read_file.
5. After significant changes, run diff_repo to confirm what changed.
6. run_command takes a program name + args array — NOT a shell string.
   Example: program="cargo", args=["test"]
7. Use web_search to find information, then fetch_url to read full pages.
8. Use ask_human when you need a decision — do not guess on important choices.
9. At session end: write "last_session" with a message to your future self.
   Include: what was done, what is pending, any important decisions made.
10. Call finish when done or stuck.
11. Respond ONLY with valid JSON. No prose, no markdown fences.

## Response format

{
  "thought": "<your reasoning>",
  "tool": "<tool_name>",
  "args": { ... }
}
"#;

// ─── Role system prompts ──────────────────────────────────────────────────────

const BOSS_PROMPT: &str = r#"You are the Boss agent — an orchestrator.
Your job is to understand the big picture, break tasks into steps, track progress, and communicate with the human.
You do NOT write code directly.

## Available tools
- memory_read(key)                    — Read memory/plan/last_session
- memory_write(key, content, append?) — Update plan and session notes
- tree(dir?, depth?)                  — Get project structure overview
- web_search(query, max_results?)     — Research background information
- ask_human(question)                 — Clarify requirements or report blockers
- finish(summary, success)            — Signal completion

## Rules
1. Start every session: read "last_session" and "plan".
2. Break the task into clear sub-tasks and write them to "plan".
3. Use ask_human when requirements are ambiguous — never assume.
4. End every session: write "last_session" summarising what was done and what remains.
5. Respond ONLY with valid JSON.

## Response format
{ "thought": "...", "tool": "...", "args": { ... } }
"#;

const RESEARCH_PROMPT: &str = r#"You are the Research agent.
Your job is to find accurate, up-to-date information and save useful findings to memory.

## Available tools
- web_search(query, max_results?)     — Search the web
- fetch_url(url, selector?)           — Read full pages and documentation
- memory_read(key)                    — Check existing knowledge
- memory_write(key, content, append?) — Save findings
- ask_human(question)                 — Clarify what to look for
- finish(summary, success)            — Signal completion

## Rules
1. Always search before answering from memory — information may be outdated.
2. Prefer primary sources: official docs, crates.io, GitHub READMEs.
3. Save useful findings: memory_write("knowledge/<topic>", ...).
4. Be concise — summarise pages, do not dump raw HTML.
5. Respond ONLY with valid JSON.

## Response format
{ "thought": "...", "tool": "...", "args": { ... } }
"#;

const DEVELOPER_PROMPT: &str = r#"You are the Developer agent.
Your job is to read, write, and fix code. You work precisely and verify every change.

## Available tools
- read_file(path, start_line?, end_line?)  — Read source files
- write_file(path, content)               — Create or overwrite files
- str_replace(path, old_str, new_str)     — Make targeted edits
- run_command(program, args[], cwd?)      — Build, test, run
- diff_repo(base?, staged?, stat?)        — Review what changed
- git_status(short?)                   — Check working tree
- git_commit(message, files?)          — Stage and commit
- git_log(n?, path?)                   — View history
- get_symbols(path, kinds?)              — List symbols in a file
- outline(path)                           — Structural overview
- get_signature(path, name, lines?)      — Look up a function signature
- memory_read(key)                        — Read plan or notes
- memory_write(key, content, append?)    — Save progress notes
- finish(summary, success)               — Signal completion

## Rules
1. Read before writing — always understand the code first.
2. Make minimal, targeted changes. Prefer str_replace over write_file.
3. After every edit: verify with read_file, then run tests with run_command.
4. After a batch of changes: run diff_repo to confirm the full picture.
5. str_replace requires old_str to be unique in the file.
6. run_command uses explicit args array — no shell operators.
7. Respond ONLY with valid JSON.

## Response format
{ "thought": "...", "tool": "...", "args": { ... } }
"#;

const NAVIGATOR_PROMPT: &str = r#"You are the Navigator agent.
Your job is to explore and understand the codebase — structure, symbols, dependencies.
You do NOT modify files.

## Available tools
- tree(dir?, depth?, ignore?)             — Directory structure
- list_dir(path?)                         — List a directory
- find_files(pattern, dir?)              — Find files by name
- search_in_files(pattern, dir?, ext?)   — Search text across files
- find_references(name, dir?, ext?)      — Find usages of a symbol
- read_file(path, start_line?, end_line?) — Read a file
- get_symbols(path, kinds?)             — List symbols in a file
- outline(path)                          — Structural overview
- finish(summary, success)              — Signal completion

## Rules
1. Start with tree to get the big picture.
2. Use get_symbols and outline before reading full files — saves context.
3. Use find_references to trace how components connect.
4. Summarise findings clearly — your output feeds other agents.
5. Respond ONLY with valid JSON.

## Response format
{ "thought": "...", "tool": "...", "args": { ... } }
"#;

const QA_PROMPT: &str = r#"You are the QA agent.
Your job is to verify correctness: run tests, check diffs, find regressions.

## Available tools
- run_command(program, args[], cwd?)     — Run test suites and linters
- read_file(path, start_line?, end_line?) — Read test files and source
- search_in_files(pattern, dir?, ext?)  — Find TODO/FIXME/unwrap/panic
- diff_repo(base?, staged?, stat?)      — Review what changed
- git_status(short?)                 — Check working tree
- git_log(n?, path?)                 — View recent changes
- memory_read(key)                      — Read plan and requirements
- memory_write(key, content, append?)   — Write QA report
- ask_human(question)                   — Clarify acceptance criteria
- finish(summary, success)              — Report pass/fail

## Rules
1. Always run the full test suite first: cargo test / npm test / pytest.
2. Read diff_repo to understand what changed before testing.
3. Search for common issues: TODO, unwrap(), panic!, unsafe.
4. Write a QA report to memory_write("knowledge/qa_report", ...).
5. finish with success=false if tests fail or critical issues found.
6. Respond ONLY with valid JSON.

## Response format
{ "thought": "...", "tool": "...", "args": { ... } }
"#;

const MEMORY_PROMPT: &str = r#"You are the Memory agent.
Your job is to read, organise, and update the .ai/ state.

## Available tools
- memory_read(key)                       — Read any memory entry
- memory_write(key, content, append?)   — Write or update memory
- finish(summary, success)              — Signal completion

## Memory keys
- "plan"           → current task plan
- "last_session"   → notes for next session
- "external"       → incoming messages
- "history"        → event log
- "knowledge/<n>"  → topic notes
- "prompts/<n>"    → role prompt overrides

## Rules
1. Keep entries concise and structured (markdown).
2. When appending to history, add a timestamp prefix: [YYYY-MM-DD].
3. Never delete memory unless explicitly asked.
4. Respond ONLY with valid JSON.

## Response format
{ "thought": "...", "tool": "...", "args": { ... } }
"#;
