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
#[command(name = "do_it", about = "do_it — autonomous coding agent (local LLM + ACI)")]
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
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::from_default_env()
                .add_directive("do_it=debug".parse()?),
        )
        .without_time()
        .init();

    // First-run init: create ~/.do_it/ with default config and system prompt
    ensure_global_config();

    let cli = Cli::parse();

    match cli.command {
        Commands::Run { task, repo, config, role, system_prompt, max_steps } => {
            // Load config: --config (explicit) > ./config.toml > ~/.do_it/config.toml > defaults
            let explicit = if config != "config.toml" { Some(config.as_str()) } else { None };
            let mut cfg = AgentConfig::load(explicit);

            // Resolve role
            let agent_role = if let Some(r) = &role {
                match Role::from_str(r) {
                    Some(role) => {
                        println!("Role: {}", role.name());
                        role
                    }
                    None => {
                        eprintln!("Unknown role '{}'. Run `do_it roles` to see available roles.", r);
                        std::process::exit(1);
                    }
                }
            } else {
                Role::Default
            };

            // --task: image path, text file, or inline string
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

            // System prompt priority: --system-prompt > --role > config.toml > builtin
            if let Some(sp) = system_prompt {
                cfg.system_prompt = load_text_or_inline(&sp, "--system-prompt")?;
                tracing::info!("System prompt: CLI override");
            } else if agent_role != Role::Default {
                // Role prompt is resolved inside agent (needs repo root for .ai/prompts/)
                // We pass the role and let agent.run() handle it
            }
            // else: cfg.system_prompt stays as loaded from config.toml

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
    }

    Ok(())
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

/// Recognised image extensions
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
