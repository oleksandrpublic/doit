mod agent;
mod config;
mod history;
mod shell;
mod tools;

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::Path;
use tracing_subscriber::EnvFilter;

use crate::agent::SweAgent;
use crate::config::{AgentConfig, Role, ensure_global_config};

#[derive(Parser)]
#[command(
    name = "do_it",
    about = "do_it — autonomous coding agent (local LLM + ACI)",
    version,
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Run the agent on a task
    Run {
        /// Task description, path to a .txt/.md file, or path to an image
        #[arg(long, short)]
        task: String,

        /// Path to the repository / working directory
        #[arg(long, short, default_value = ".")]
        repo: String,

        /// Path to config file
        #[arg(long, short, default_value = "config.toml")]
        config: String,

        /// Agent role — restricts tool set and sets role-specific system prompt.
        /// Roles: boss, research, developer, navigator, qa, reviewer, memory
        /// (default: unrestricted)
        #[arg(long)]
        role: Option<String>,

        /// Override system prompt: inline text or path to a .txt/.md file.
        /// Takes precedence over --role prompt and config.toml.
        #[arg(long)]
        system_prompt: Option<String>,

        /// Max agent steps
        #[arg(long, default_value = "30")]
        max_steps: u32,
    },

    /// Print resolved config and exit
    Config {
        #[arg(long, default_value = "config.toml")]
        config: String,
    },

    /// List available roles with their tool allowlists
    Roles,

    /// Show current project status: last session, plan, wishlist, session logs
    Status {
        /// Path to the repository / working directory
        #[arg(long, short, default_value = ".")]
        repo: String,
    },

    /// Initialise .ai/ workspace in the current (or given) directory
    Init {
        /// Path to the repository / working directory
        #[arg(long, short, default_value = ".")]
        repo: String,

        /// Ollama model to use (default: ask interactively, or "qwen3.5:cloud")
        #[arg(long)]
        model: Option<String>,

        /// Skip interactive prompts — use all defaults
        #[arg(long, short)]
        yes: bool,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_ansi(false)
        .with_env_filter(
            EnvFilter::from_default_env()
                .add_directive("do_it=debug".parse()?),
        )
        .without_time()
        .init();

    // First-run: create ~/.do_it/ with default global config
    ensure_global_config();

    let cli = Cli::parse();

    match cli.command {
        Commands::Run { task, repo, config, role, system_prompt, max_steps } => {
            let explicit = if config != "config.toml" { Some(config.as_str()) } else { None };
            let mut cfg = AgentConfig::load(explicit);

            let agent_role = resolve_role(role.as_deref())?;

            let task_image: Option<std::path::PathBuf> = if is_image_path(&task) {
                tracing::info!("--task: image detected '{}'", task);
                Some(std::path::PathBuf::from(&task))
            } else {
                None
            };
            let task_text = if task_image.is_some() {
                String::new()
            } else {
                load_text_or_inline(&task, "--task")?
            };

            if let Some(sp) = system_prompt {
                cfg.system_prompt = load_text_or_inline(&sp, "--system-prompt")?;
                tracing::info!("System prompt: CLI override");
            }

            tracing::info!("Model: {} | Repo: {} | Role: {}", cfg.model, repo, agent_role.name());
            let mut agent = SweAgent::new(cfg, &repo, max_steps as usize, agent_role)?;
            agent.run(&task_text, task_image).await?;
        }

        Commands::Config { config } => {
            let cfg = AgentConfig::load_or_default(&config);
            println!("{}", toml::to_string_pretty(&cfg)?);
        }

        Commands::Roles => {
            print_roles();
        }

        Commands::Status { repo } => {
            cmd_status(&repo)?;
        }

        Commands::Init { repo, model, yes } => {
            cmd_init(&repo, model.as_deref(), yes)?;
        }
    }

    Ok(())
}

// ─── do_it status ─────────────────────────────────────────────────────────────

fn cmd_status(repo: &str) -> Result<()> {
    let root = std::path::PathBuf::from(repo)
        .canonicalize()
        .unwrap_or_else(|_| std::path::PathBuf::from(repo));
    let ai = root.join(".ai");

    println!("╭─ do_it status ─────────────────────────────────────────────╮");
    println!("│ repo: {}", root.display());
    println!("╰────────────────────────────────────────────────────────────╯");
    println!();

    if !ai.exists() {
        println!("  ⚠  No .ai/ workspace found.");
        println!("     Run `do_it init` to initialise one.");
        return Ok(());
    }

    // ── Session counter ────────────────────────────────────────────────────
    let counter_path = ai.join("state/session_counter.txt");
    let session_n: u64 = std::fs::read_to_string(&counter_path)
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0);
    println!("  Sessions run: {session_n}");

    // ── Session logs ───────────────────────────────────────────────────────
    let logs_dir = ai.join("logs");
    let mut session_logs: Vec<std::path::PathBuf> = std::fs::read_dir(&logs_dir)
        .ok()
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with("session-") && n.ends_with(".md"))
                .unwrap_or(false)
        })
        .collect();
    session_logs.sort();

    if session_logs.is_empty() {
        println!("  Session logs: none");
    } else {
        println!("  Session logs ({} total):", session_logs.len());
        // Show last 5
        for log in session_logs.iter().rev().take(5) {
            let name = log.file_name().unwrap_or_default().to_string_lossy();
            let size = std::fs::metadata(log).map(|m| m.len()).unwrap_or(0);
            println!("    • {} ({} bytes)", name, size);
        }
        if session_logs.len() > 5 {
            println!("    … and {} older logs", session_logs.len() - 5);
        }
    }

    println!();

    // ── Last session ───────────────────────────────────────────────────────
    let last_session_path = ai.join("state/last_session.md");
    println!("── Last session ──────────────────────────────────────────────");
    match std::fs::read_to_string(&last_session_path) {
        Ok(content) if !content.trim().is_empty() => {
            let lines: Vec<&str> = content.lines().collect();
            for line in lines.iter().take(40) {
                println!("  {line}");
            }
            if lines.len() > 40 {
                println!("  … ({} more lines)", lines.len() - 40);
            }
        }
        _ => println!("  (no last_session.md — first run or cleared)"),
    }
    println!();

    // ── Plan ───────────────────────────────────────────────────────────────
    let plan_path = ai.join("state/current_plan.md");
    println!("── Current plan ──────────────────────────────────────────────");
    match std::fs::read_to_string(&plan_path) {
        Ok(content) if !content.trim().is_empty() => {
            let count = content.lines().count();
            for line in content.lines().take(30) {
                println!("  {line}");
            }
            if count > 30 {
                println!("  … (see .ai/state/current_plan.md for full plan)");
            }
        }
        _ => println!("  (no plan yet)"),
    }
    println!();

    // ── Tool wishlist ──────────────────────────────────────────────────────
    println!("── Tool wishlist (~/.do_it/tool_wishlist.md) ─────────────────");
    let wishlist = config::global_tool_wishlist_path()
        .and_then(|p| std::fs::read_to_string(&p).ok());
    match wishlist {
        Some(content) if !content.trim().is_empty() => {
            let entries: Vec<&str> = content
                .split("\n## ")
                .filter(|s| !s.trim().is_empty())
                .collect();
            println!("  {} request(s) total", entries.len());
            for entry in entries.iter().rev().take(3) {
                let title = entry.lines().next()
                    .unwrap_or("")
                    .trim_start_matches("## ")
                    .trim();
                println!("  • {title}");
            }
            if entries.len() > 3 {
                println!("  … (see ~/.do_it/tool_wishlist.md for full list)");
            }
        }
        _ => println!("  (empty)"),
    }
    println!();

    // ── Knowledge keys ─────────────────────────────────────────────────────
    let knowledge_dir = ai.join("knowledge");
    if knowledge_dir.exists() {
        let mut keys: Vec<String> = std::fs::read_dir(&knowledge_dir)
            .ok()
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| {
                let p = e.path();
                if p.extension().and_then(|x| x.to_str()) == Some("md") {
                    p.file_stem().and_then(|s| s.to_str()).map(String::from)
                } else {
                    None
                }
            })
            .collect();
        keys.sort();

        if !keys.is_empty() {
            println!("── Knowledge keys (.ai/knowledge/) ───────────────────────────");
            for key in &keys {
                println!("  • {key}");
            }
            println!();
        }
    }

    Ok(())
}

// ─── do_it init ───────────────────────────────────────────────────────────────

const AI_SUBDIRS: &[&str] = &["state", "logs", "knowledge", "prompts", "tools", "screenshots"];

fn cmd_init(repo: &str, model_arg: Option<&str>, yes: bool) -> Result<()> {
    let root = std::path::PathBuf::from(repo);
    let ai = root.join(".ai");

    let display_root = root.canonicalize().unwrap_or_else(|_| root.clone());
    println!("╭─ do_it init ────────────────────────────────────────────────╮");
    println!("│ repo: {}", display_root.display());
    println!("╰────────────────────────────────────────────────────────────╯");
    println!();

    // ── Create .ai/ directory structure ───────────────────────────────────
    for sub in AI_SUBDIRS {
        let dir = ai.join(sub);
        let existed = dir.exists();
        std::fs::create_dir_all(&dir)
            .map_err(|e| anyhow::anyhow!("Cannot create .ai/{sub}/: {e}"))?;
        if existed {
            println!("  ✓ exists   .ai/{sub}/");
        } else {
            println!("  ✓ created  .ai/{sub}/");
        }
    }
    println!();

    // ── Resolve model ──────────────────────────────────────────────────────
    let model = if let Some(m) = model_arg {
        m.to_string()
    } else if yes {
        "qwen3.5:cloud".to_string()
    } else {
        prompt_input("Ollama model to use", "qwen3.5:cloud")
    };

    // ── Write config.toml ─────────────────────────────────────────────────
    let config_path = root.join("config.toml");
    if config_path.exists() {
        println!("  ✓ exists   config.toml (not overwritten)");
    } else {
        let config_content = format!(
            r#"# do_it configuration — generated by `do_it init`

ollama_base_url = "http://localhost:11434"
model = "{model}"
temperature = 0.0
max_tokens = 4096
history_window = 8
max_output_chars = 6000
max_depth = 3
system_prompt = """
You are an autonomous software engineering agent running on a developer machine.
Your goal is to solve programming tasks by using a set of tools to interact with the filesystem and shell.

## Available tools

- read_file(path, start_line?, end_line?)        — View a file with line numbers
- write_file(path, content)                       — Overwrite a file completely
- str_replace(path, old_str, new_str)             — Replace a unique string in a file
- list_dir(path?)                                 — List directory contents
- find_files(pattern, dir?)                       — Find files by name/glob (cross-platform)
- search_in_files(pattern, dir?, ext?)            — Search text across files
- run_command(program, args[], cwd?)              — Run an executable with explicit args (cross-platform, no shell)
- finish(summary, success)                        — Signal completion

## Rules

1. Explore before editing: use list_dir and read_file first.
2. Make minimal, targeted changes.
3. After editing, verify with read_file.
4. run_command takes a program name + args array — NOT a shell string.
   - Example: program="cargo", args=["test"]
5. Call finish when done or when you cannot make further progress.
6. Respond ONLY with valid JSON. No prose, no markdown fences.

## Response format

{{
  "thought": "<your reasoning>",
  "tool": "<tool_name>",
  "args": {{ ... }}
}}
"""

# Optional: different models per role
# [models]
# thinking = "qwen3.5:9b"
# coding   = "qwen3-coder-next"
# search    = "qwen3.5:4b"
# execution = "qwen3.5:4b"
# vision    = "qwen3.5:9b"

# Optional: Telegram for ask_human and notify
# telegram_token   = "BOT_TOKEN"
# telegram_chat_id = "CHAT_ID"

# Optional: Browser backend
# [browser]
# cdp_url        = "ws://127.0.0.1:9222"
# chrome_path    = ""
# screenshot_dir = ".ai/screenshots"
"#,
            model = model
        );
        std::fs::write(&config_path, config_content)
            .map_err(|e| anyhow::anyhow!("Cannot write config.toml: {e}"))?;
        println!("  ✓ created  config.toml  (model: {model})");
    }

    // ── Write .ai/project.toml ────────────────────────────────────────────
    let project_path = ai.join("project.toml");
    if project_path.exists() {
        println!("  ✓ exists   .ai/project.toml (not overwritten)");
    } else {
        let project_name = if yes {
            root.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("my-project")
                .to_string()
        } else {
            prompt_input(
                "Project name",
                root.file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("my-project"),
            )
        };

        let project_content = format!(
            r#"# do_it project configuration

[project]
name        = "{project_name}"
description = ""
language    = ""      # e.g. "Rust", "TypeScript", "Python"
framework   = ""      # e.g. "Axum", "React", "FastAPI"

[build]
build   = ""          # e.g. "cargo build"
test    = ""          # e.g. "cargo test"
lint    = ""          # e.g. "cargo clippy"
serve   = ""          # e.g. "cargo run"

[conventions]
# code_style   = "..."
# commit_style = "conventional commits"
"#
        );
        std::fs::write(&project_path, project_content)
            .map_err(|e| anyhow::anyhow!("Cannot write .ai/project.toml: {e}"))?;
        println!("  ✓ created  .ai/project.toml");
    }

    // ── .gitignore ────────────────────────────────────────────────────────
    let gitignore_path = root.join(".gitignore");
    let ai_entries = [
        ".ai/state/",
        ".ai/logs/",
        ".ai/screenshots/",
    ];

    if gitignore_path.exists() {
        let existing = std::fs::read_to_string(&gitignore_path).unwrap_or_default();
        let missing: Vec<&str> = ai_entries.iter()
            .copied()
            .filter(|e| !existing.contains(e))
            .collect();

        if missing.is_empty() {
            println!("  ✓ exists   .gitignore (do_it entries already present)");
        } else {
            let mut f = std::fs::OpenOptions::new()
                .append(true)
                .open(&gitignore_path)
                .map_err(|e| anyhow::anyhow!("Cannot open .gitignore: {e}"))?;
            use std::io::Write as _;
            writeln!(f, "\n# do_it — runtime state (do not commit)")?;
            for entry in &missing {
                writeln!(f, "{entry}")?;
            }
            println!("  ✓ updated  .gitignore (added: {})", missing.join(", "));
        }
    } else {
        let content = format!(
            "# do_it — runtime state (do not commit)\n{}\n",
            ai_entries.join("\n")
        );
        std::fs::write(&gitignore_path, content)
            .map_err(|e| anyhow::anyhow!("Cannot write .gitignore: {e}"))?;
        println!("  ✓ created  .gitignore");
    }

    println!();
    println!("  Workspace ready.");
    println!("  Next step:");
    println!("    do_it run --task \"describe your task here\" --role boss");
    println!();

    Ok(())
}

/// Simple stdin prompt with a default value shown in brackets.
fn prompt_input(label: &str, default: &str) -> String {
    use std::io::Write as _;
    print!("  {label} [{}]: ", default);
    std::io::stdout().flush().ok();
    let mut line = String::new();
    std::io::stdin().read_line(&mut line).ok();
    let trimmed = line.trim();
    if trimmed.is_empty() { default.to_string() } else { trimmed.to_string() }
}

// ─── Shared helpers ────────────────────────────────────────────────────────────

fn resolve_role(role: Option<&str>) -> Result<Role> {
    match role {
        None => Ok(Role::Default),
        Some(r) => Role::from_str(r).ok_or_else(|| {
            eprintln!("Unknown role '{}'. Run `do_it roles` to see available roles.", r);
            anyhow::anyhow!("Unknown role: {r}")
        }),
    }
}

fn print_roles() {
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

    println!("Available roles:\n");
    for role in &roles {
        let tools = role.allowed_tools();
        let tool_str = if tools.is_empty() {
            "(all tools)".to_string()
        } else {
            tools.join(", ")
        };
        println!("  {:12} — {}", role.name(), tool_str);
    }
    println!("\nUsage:  do_it run --task \"...\" --role developer");
    println!("Prompt: .ai/prompts/<role>.md overrides built-in prompt");
}

const IMAGE_EXTS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "bmp"];

fn is_image_path(value: &str) -> bool {
    let p = Path::new(value);
    if !p.exists() { return false; }
    p.extension()
        .and_then(|e| e.to_str())
        .map(|e| IMAGE_EXTS.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

fn load_text_or_inline(value: &str, arg_name: &str) -> Result<String> {
    let p = Path::new(value);
    if p.exists() && p.is_file() && !is_image_path(value) {
        let content = std::fs::read_to_string(p)
            .map_err(|e| anyhow::anyhow!("{arg_name}: failed to read '{}': {e}", p.display()))?;
        tracing::info!("{arg_name}: loaded from file '{}'", p.display());
        Ok(content.trim_end().to_string())
    } else {
        Ok(value.to_string())
    }
}
