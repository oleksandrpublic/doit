use crate::config_struct::{AgentConfig, PROMPT_DEFAULT};
use std::path::{Path, PathBuf};

pub struct LoadedConfig {
    pub config: AgentConfig,
    pub source: String,
}

impl AgentConfig {
    /// Load config following the priority chain:
    ///   --config <path>       — explicit CLI override
    ///   <repo>/config.toml    — local project config for the selected repo
    ///   ./config.toml         — local project config in the current directory
    ///   ~/.do_it/config.toml  — global user config
    ///   built-in defaults
    pub fn load(explicit: Option<&str>) -> Self {
        Self::load_for_repo(explicit.map(Path::new), None)
    }

    /// Load config using the repository root when resolving the default config.toml.
    pub fn load_for_repo(explicit: Option<&Path>, repo_root: Option<&Path>) -> Self {
        Self::load_for_repo_with_source(explicit, repo_root).config
    }

    pub fn load_for_repo_with_source(
        explicit: Option<&Path>,
        repo_root: Option<&Path>,
    ) -> LoadedConfig {
        if let Some(path) = explicit {
            let source = format!("explicit: {}", path.display());
            return Self::load_file(path)
                .map(|config| LoadedConfig { config, source })
                .unwrap_or_else(|| {
                    report_config_warning(format!(
                        "Config '{}' not found or invalid, using defaults",
                        path.display()
                    ));
                    LoadedConfig {
                        config: Self::default(),
                        source: "built-in defaults".to_string(),
                    }
                });
        }

        if let Some(root) = repo_root {
            let repo_config = root.join("config.toml");
            if repo_config.exists() {
                if let Some(cfg) = Self::load_file(&repo_config) {
                    tracing::info!("Config: {}", repo_config.display());
                    return LoadedConfig {
                        config: cfg,
                        source: format!("repo: {}", repo_config.display()),
                    };
                }
            }
        } else if Path::new("config.toml").exists() {
            if let Some(cfg) = Self::load_file(Path::new("config.toml")) {
                tracing::info!("Config: ./config.toml");
                return LoadedConfig {
                    config: cfg,
                    source: "cwd: ./config.toml".to_string(),
                };
            }
        }

        if let Some(global) = global_config_path() {
            if global.exists() {
                if let Some(cfg) = Self::load_file(&global) {
                    tracing::info!("Config: {}", global.display());
                    return LoadedConfig {
                        config: cfg,
                        source: format!("global: {}", global.display()),
                    };
                }
            }
        }

        tracing::info!("Config: built-in defaults");
        LoadedConfig {
            config: Self::default(),
            source: "built-in defaults".to_string(),
        }
    }

    /// Legacy alias — kept for compatibility.
    pub fn load_or_default(path: &str) -> Self {
        Self::load_or_default_with_source(path).config
    }

    pub fn load_or_default_with_source(path: &str) -> LoadedConfig {
        let source = format!("explicit: {path}");
        Self::load_file(Path::new(path))
            .map(|config| LoadedConfig { config, source })
            .unwrap_or_else(|| {
                report_config_warning(format!(
                    "Config '{}' not found or invalid, using defaults",
                    path
                ));
                LoadedConfig {
                    config: Self::default(),
                    source: "built-in defaults".to_string(),
                }
            })
    }

    fn load_file(path: &Path) -> Option<Self> {
        if !path.exists() {
            return None;
        }

        let raw = match std::fs::read_to_string(path) {
            Ok(raw) => raw,
            Err(e) => {
                report_config_error(format!("Config read error '{}': {e}", path.display()));
                return None;
            }
        };

        let parsed: toml::Value = match toml::from_str(&raw) {
            Ok(parsed) => parsed,
            Err(e) => {
                report_config_error(format!("Config parse error in '{}': {e}", path.display()));
                return None;
            }
        };

        let mut merged = match toml::Value::try_from(Self::default()) {
            Ok(value) => value,
            Err(e) => {
                report_config_error(format!(
                    "Failed to build default config while loading '{}': {e}",
                    path.display()
                ));
                return None;
            }
        };

        merge_toml_values(&mut merged, parsed);

        let mut cfg: Self = match merged.try_into() {
            Ok(cfg) => cfg,
            Err(e) => {
                report_config_error(format!("Config decode error in '{}': {e}", path.display()));
                return None;
            }
        };

        // Apply ~/.do_it/system_prompt.md if config still has the default prompt.
        if cfg.system_prompt == PROMPT_DEFAULT {
            if let Some(sp) = load_global_system_prompt() {
                cfg.system_prompt = sp;
            }
        }

        Some(cfg)
    }
}

fn merge_toml_values(base: &mut toml::Value, overlay: toml::Value) {
    match (base, overlay) {
        (toml::Value::Table(base_table), toml::Value::Table(overlay_table)) => {
            for (key, overlay_value) in overlay_table {
                if let Some(base_value) = base_table.get_mut(&key) {
                    merge_toml_values(base_value, overlay_value);
                } else {
                    base_table.insert(key, overlay_value);
                }
            }
        }
        (base_slot, overlay_value) => {
            *base_slot = overlay_value;
        }
    }
}

fn report_config_warning(message: String) {
    eprintln!("{message}");
    tracing::warn!("{}", message);
}

fn report_config_error(message: String) {
    eprintln!("{message}");
    tracing::error!("{}", message);
}

// ─── Global config helpers ────────────────────────────────────────────────────

/// Returns ~/.do_it/ as a PathBuf, or None if home dir cannot be determined.
pub fn global_config_dir() -> Option<PathBuf> {
    home_dir().map(|h| h.join(".do_it"))
}

/// Returns ~/.do_it/config.toml
pub fn global_config_path() -> Option<PathBuf> {
    global_config_dir().map(|d| d.join("config.toml"))
}

/// Returns ~/.do_it/user_profile.md
pub fn global_user_profile_path() -> Option<PathBuf> {
    global_config_dir().map(|d| d.join("user_profile.md"))
}

/// Returns ~/.do_it/boss_notes.md
pub fn global_boss_notes_path() -> Option<PathBuf> {
    global_config_dir().map(|d| d.join("boss_notes.md"))
}

/// Returns ~/.do_it/tool_wishlist.md
pub fn global_tool_wishlist_path() -> Option<PathBuf> {
    global_config_dir().map(|d| d.join("tool_wishlist.md"))
}

/// Load ~/.do_it/system_prompt.md if it exists and is non-empty.
pub fn load_global_system_prompt() -> Option<String> {
    let path = global_config_dir()?.join("system_prompt.md");
    std::fs::read_to_string(&path)
        .ok()
        .filter(|s| !s.trim().is_empty())
}

/// Cross-platform home directory.
fn home_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        std::env::var("USERPROFILE")
            .ok()
            .or_else(|| {
                let drive = std::env::var("HOMEDRIVE").ok()?;
                let path = std::env::var("HOMEPATH").ok()?;
                Some(format!("{drive}{path}"))
            })
            .map(PathBuf::from)
    }
    #[cfg(not(windows))]
    {
        std::env::var("HOME").ok().map(PathBuf::from)
    }
}

/// Ensure `user_profile.md` and `boss_notes.md` exist in `~/.do_it/`.
///
/// Called at `session_init` for installations that pre-date the template
/// scaffolding added in Sprint 2. Safe to call repeatedly: does not
/// overwrite files that already exist, even if they are empty.
pub fn ensure_memory_files_exist() {
    let dir = match global_config_dir() {
        Some(d) => d,
        None => return,
    };
    if !dir.exists() {
        // Directory will be created by ensure_global_config() on the next
        // explicit first-run path; we don't create it here to avoid
        // surprising the user with a hidden directory on every session_init.
        return;
    }

    let memory_files: &[(&str, &str)] = &[
        (
            "user_profile.md",
            "# User profile\n\
             #\n\
             # The Boss agent reads this file at the start of every session.\n\
             # Describe your preferences so the agent works the way you like.\n\
             #\n\
             # Suggested sections:\n\
             #\n\
             # ## Communication\n\
             # - Preferred language: English\n\
             # - Response style: concise, technical\n\
             #\n\
             # ## Development stack\n\
             # - Primary language: Rust\n\
             # - Architecture notes: isolated Cargo workspaces\n\
             #\n\
             # ## Workflow preferences\n\
             # - Prefer full rewrites over patch accumulation when fixes cause regressions\n\
             # - Always run clippy before committing\n",
        ),
        (
            "boss_notes.md",
            "# Boss notes\n\
             #\n\
             # Cross-project insights accumulated by the Boss agent.\n\
             # The Boss appends here when it discovers something worth keeping beyond the current project.\n\
             #\n",
        ),
    ];

    for (name, template) in memory_files {
        let path = dir.join(name);
        if !path.exists() {
            if let Err(e) = std::fs::write(&path, template) {
                tracing::warn!("ensure_memory_files_exist: could not create {}: {e}", path.display());
            } else {
                tracing::info!("ensure_memory_files_exist: created {}", path.display());
            }
        }
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
        (
            "config.toml",
            r#"# do_it — global configuration
# Used when no local ./config.toml is found.
# Local config.toml always takes precedence.

# ── LLM backend ───────────────────────────────────────────────────────────────
#
# llm_backend selects the wire protocol:
#   "ollama"    — Ollama  (default, local, no key needed)
#   "openai"    — OpenAI-compatible API (OpenAI, MiniMax, local proxies, …)
#   "anthropic" — Anthropic-compatible API (Anthropic, MiniMax, …)
#
# llm_api_key can also be supplied via the LLM_API_KEY environment variable.
#
# Examples:
#
#   # Local Ollama (default)
#   llm_backend = "ollama"
#   llm_url     = "http://localhost:11434"
#
#   # OpenAI
#   llm_backend = "openai"
#   llm_url     = "https://api.openai.com"
#   llm_api_key = "sk-..."
#   model       = "gpt-4o"
#
#   # Anthropic
#   llm_backend = "anthropic"
#   llm_url     = "https://api.anthropic.com"
#   llm_api_key = "sk-ant-..."
#   model       = "claude-sonnet-4-5-20251001"
#
#   # MiniMax via OpenAI-compatible API
#   llm_backend = "openai"
#   llm_url     = "https://api.minimax.io"
#   llm_api_key = "..."
#   model       = "abab6.5s-chat"

llm_backend      = "ollama"
llm_url          = "http://localhost:11434"
# llm_api_key    = ""

model            = "qwen3.5:cloud"
temperature      = 0.0
max_tokens       = 4096
history_window   = 8
max_output_chars = 6000

# ── Per-task-type model overrides ─────────────────────────────────────────────
[models]
# thinking  = "qwen3.5:cloud"
# coding    = "qwen3-coder-next:cloud"
# search    = "qwen3.5:9b"
# execution = "qwen3.5:9b"
# vision    = "qwen3.5:cloud"

# ── Telegram (optional) ───────────────────────────────────────────────────────
# telegram_token   = "1234567890:ABCdef..."
# telegram_chat_id = "123456789"
"#,
        ),
        (
            "system_prompt.md",
            r#"# Global system prompt override for do_it
#
# Delete these comments and write your own prompt to activate this file.
# When active, this file overrides the built-in default prompt.
# Role-specific prompts (boss, developer, etc.) are NOT affected by this file.
"#,
        ),
        (
            "user_profile.md",
            r#"# User profile
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
"#,
        ),
        (
            "boss_notes.md",
            r#"# Boss notes
#
# Cross-project insights accumulated by the Boss agent.
# The Boss appends here when it discovers something worth keeping beyond the current project.
#
"#,
        ),
        (
            "tool_wishlist.md",
            r#"# Tool wishlist
#
# Agent-requested capabilities, written automatically by the Boss via tool_request and capability_gap.
# Each entry describes a missing tool or observed limitation encountered during real tasks.
# Review this file to prioritise new tool development.
#
"#,
        ),
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

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use tempfile::TempDir;

    fn write_file(path: &Path, contents: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, contents).unwrap();
    }

    #[test]
    fn load_for_repo_prefers_explicit_config_over_repo_config() {
        let temp = TempDir::new().unwrap();
        let repo_root = temp.path().join("repo");
        std::fs::create_dir_all(&repo_root).unwrap();

        let explicit = temp.path().join("explicit.toml");
        write_file(
            &explicit,
            r#"
model = "explicit-model"
temperature = 0.2
"#,
        );
        write_file(
            &repo_root.join("config.toml"),
            r#"
model = "repo-model"
temperature = 0.7
"#,
        );

        let cfg = AgentConfig::load_for_repo(Some(&explicit), Some(&repo_root));
        assert_eq!(cfg.model, "explicit-model");
        assert!((cfg.temperature - 0.2).abs() < f64::EPSILON);
    }

    #[test]
    fn load_for_repo_with_source_reports_explicit_source() {
        let temp = TempDir::new().unwrap();
        let repo_root = temp.path().join("repo");
        std::fs::create_dir_all(&repo_root).unwrap();

        let explicit = temp.path().join("explicit.toml");
        write_file(&explicit, "model = \"explicit-model\"\n");

        let loaded = AgentConfig::load_for_repo_with_source(Some(&explicit), Some(&repo_root));

        assert_eq!(loaded.config.model, "explicit-model");
        assert_eq!(loaded.source, format!("explicit: {}", explicit.display()));
    }

    #[test]
    #[serial]
    fn load_for_repo_prefers_repo_config_over_global_config() {
        let temp = TempDir::new().unwrap();
        let home = temp.path().join("home");
        let repo_root = temp.path().join("repo");
        std::fs::create_dir_all(&repo_root).unwrap();

        let previous_userprofile = std::env::var("USERPROFILE").ok();
        std::env::set_var("USERPROFILE", &home);

        write_file(
            &home.join(".do_it").join("config.toml"),
            r#"
model = "global-model"
temperature = 0.9
"#,
        );
        write_file(
            &repo_root.join("config.toml"),
            r#"
model = "repo-model"
temperature = 0.3
"#,
        );

        let cfg = AgentConfig::load_for_repo(None, Some(&repo_root));

        match previous_userprofile {
            Some(value) => std::env::set_var("USERPROFILE", value),
            None => std::env::remove_var("USERPROFILE"),
        }

        assert_eq!(cfg.model, "repo-model");
        assert!((cfg.temperature - 0.3).abs() < f64::EPSILON);
    }

    #[test]
    #[serial]
    fn load_for_repo_with_source_reports_repo_source_before_global() {
        let temp = TempDir::new().unwrap();
        let home = temp.path().join("home");
        let repo_root = temp.path().join("repo");
        std::fs::create_dir_all(&repo_root).unwrap();

        let previous_userprofile = std::env::var("USERPROFILE").ok();
        std::env::set_var("USERPROFILE", &home);

        write_file(
            &home.join(".do_it").join("config.toml"),
            "model = \"global-model\"\n",
        );
        write_file(&repo_root.join("config.toml"), "model = \"repo-model\"\n");

        let loaded = AgentConfig::load_for_repo_with_source(None, Some(&repo_root));

        match previous_userprofile {
            Some(value) => std::env::set_var("USERPROFILE", value),
            None => std::env::remove_var("USERPROFILE"),
        }

        assert_eq!(loaded.config.model, "repo-model");
        assert_eq!(
            loaded.source,
            format!("repo: {}", repo_root.join("config.toml").display())
        );
    }

    #[test]
    #[serial]
    fn global_system_prompt_applies_only_when_config_keeps_default_prompt() {
        let temp = TempDir::new().unwrap();
        let home = temp.path().join("home");
        let repo_root = temp.path().join("repo");
        std::fs::create_dir_all(&repo_root).unwrap();

        let previous_userprofile = std::env::var("USERPROFILE").ok();
        std::env::set_var("USERPROFILE", &home);

        write_file(
            &home.join(".do_it").join("system_prompt.md"),
            "Global prompt override",
        );

        write_file(
            &repo_root.join("config.toml"),
            r#"
model = "repo-model"
"#,
        );
        let cfg_with_default_prompt = AgentConfig::load_for_repo(None, Some(&repo_root));
        assert_eq!(
            cfg_with_default_prompt.system_prompt,
            "Global prompt override"
        );

        write_file(
            &repo_root.join("config.toml"),
            r#"
model = "repo-model"
system_prompt = "Repo prompt override"
"#,
        );
        let cfg_with_repo_prompt = AgentConfig::load_for_repo(None, Some(&repo_root));

        match previous_userprofile {
            Some(value) => std::env::set_var("USERPROFILE", value),
            None => std::env::remove_var("USERPROFILE"),
        }

        assert_eq!(cfg_with_repo_prompt.system_prompt, "Repo prompt override");
    }

    #[test]
    #[serial]
    fn load_for_repo_with_source_falls_back_to_built_in_defaults_when_nothing_present() {
        let temp = TempDir::new().unwrap();
        let empty_repo = temp.path().join("empty_repo");
        std::fs::create_dir_all(&empty_repo).unwrap();

        let fake_home = temp.path().join("fake_home");
        std::fs::create_dir_all(&fake_home).unwrap();
        let previous_userprofile = std::env::var("USERPROFILE").ok();
        std::env::set_var("USERPROFILE", &fake_home);

        let loaded = AgentConfig::load_for_repo_with_source(None, Some(&empty_repo));

        match previous_userprofile {
            Some(value) => std::env::set_var("USERPROFILE", value),
            None => std::env::remove_var("USERPROFILE"),
        }

        assert_eq!(
            loaded.source, "built-in defaults",
            "source must be 'built-in defaults' when no config file exists anywhere"
        );
        assert_eq!(
            loaded.config.model,
            AgentConfig::default().model,
            "config must equal AgentConfig::default() when no config file exists"
        );
    }

    #[test]
    fn load_or_default_with_source_returns_defaults_for_nonexistent_path() {
        let loaded =
            AgentConfig::load_or_default_with_source("/tmp/this_config_does_not_exist_xyz.toml");

        assert_eq!(
            loaded.source, "built-in defaults",
            "source must be 'built-in defaults' for a missing path"
        );
        assert_eq!(
            loaded.config.model,
            AgentConfig::default().model,
            "config must equal AgentConfig::default() for a missing path"
        );
    }

    // ── ensure_memory_files_exist ─────────────────────────────────────────────

    #[test]
    #[serial]
    fn ensure_memory_files_creates_missing_files() {
        let temp = TempDir::new().unwrap();
        let home = temp.path().join("home");
        // Create ~/.do_it/ but leave memory files absent
        let dot_do_it = home.join(".do_it");
        std::fs::create_dir_all(&dot_do_it).unwrap();

        let previous_userprofile = std::env::var("USERPROFILE").ok();
        std::env::set_var("USERPROFILE", &home);

        ensure_memory_files_exist();

        match previous_userprofile {
            Some(value) => std::env::set_var("USERPROFILE", value),
            None => std::env::remove_var("USERPROFILE"),
        }

        assert!(dot_do_it.join("user_profile.md").exists(), "user_profile.md must be created");
        assert!(dot_do_it.join("boss_notes.md").exists(), "boss_notes.md must be created");
    }

    #[test]
    #[serial]
    fn ensure_memory_files_does_not_overwrite_existing() {
        let temp = TempDir::new().unwrap();
        let home = temp.path().join("home");
        let dot_do_it = home.join(".do_it");
        std::fs::create_dir_all(&dot_do_it).unwrap();

        let user_profile_path = dot_do_it.join("user_profile.md");
        std::fs::write(&user_profile_path, "my custom profile content").unwrap();

        let previous_userprofile = std::env::var("USERPROFILE").ok();
        std::env::set_var("USERPROFILE", &home);

        ensure_memory_files_exist();

        match previous_userprofile {
            Some(value) => std::env::set_var("USERPROFILE", value),
            None => std::env::remove_var("USERPROFILE"),
        }

        let content = std::fs::read_to_string(&user_profile_path).unwrap();
        assert_eq!(content, "my custom profile content", "existing file must not be overwritten");
    }

    #[test]
    #[serial]
    fn ensure_memory_files_noop_when_dot_do_it_absent() {
        let temp = TempDir::new().unwrap();
        let home = temp.path().join("home_no_dot_do_it");
        // Do NOT create ~/.do_it — function should return silently
        std::fs::create_dir_all(&home).unwrap();

        let previous_userprofile = std::env::var("USERPROFILE").ok();
        std::env::set_var("USERPROFILE", &home);

        ensure_memory_files_exist(); // must not panic or create the directory

        match previous_userprofile {
            Some(value) => std::env::set_var("USERPROFILE", value),
            None => std::env::remove_var("USERPROFILE"),
        }

        assert!(!home.join(".do_it").exists(), ".do_it must not be created");
    }
}
