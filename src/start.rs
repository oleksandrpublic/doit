use crate::agent::SweAgent;
use crate::config_loader::LoadedConfig;
use crate::config_struct::AgentConfig;
use clap::{Parser, Subcommand};
use std::path::Path;

#[path = "start/init.rs"]
mod init;
#[path = "start/check.rs"]
mod check;
#[path = "start/run_support.rs"]
mod run_support;
#[path = "start/shared.rs"]
mod shared;
#[path = "start/status.rs"]
mod status;

#[cfg(test)]
#[path = "start/tests.rs"]
mod tests;

pub use init::cmd_init;
pub use check::cmd_check;
pub use shared::{
    check_dependencies, ensure_project_prompts, is_image_path, load_task_text_or_inline,
    load_text_or_inline, print_roles, resolve_role, setup_logging,
};
pub use status::cmd_status;

static CONSOLE_LOGS_ENABLED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

pub fn set_console_logs_enabled(enabled: bool) {
    CONSOLE_LOGS_ENABLED.store(enabled, std::sync::atomic::Ordering::Relaxed);
}

pub fn console_logs_enabled() -> bool {
    CONSOLE_LOGS_ENABLED.load(std::sync::atomic::Ordering::Relaxed)
}

#[derive(Parser)]
#[command(
    name = "do_it",
    about = "do_it — autonomous coding agent (local LLM + ACI)",
    version
)]
pub struct Cli {
    #[command(subcommand)]
    command: Commands,
}

impl Cli {
    fn requires_global_config_init(&self) -> bool {
        matches!(&self.command, Commands::Run { .. })
    }
}

#[derive(Subcommand)]
pub enum Commands {
    /// Run the agent on a task
    Run {
        #[arg(long, short)]
        task: String,
        #[arg(long, short, default_value = ".")]
        repo: String,
        #[arg(long, short, default_value = "config.toml")]
        config: String,
        #[arg(long)]
        role: Option<String>,
        #[arg(long)]
        system_prompt: Option<String>,
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
        #[arg(long, short, default_value = ".")]
        repo: String,
    },

    /// Dry-run validation: config load, model reachability, and .ai/ structure
    Check {
        #[arg(long, short, default_value = ".")]
        repo: String,
        #[arg(long, short, default_value = "config.toml")]
        config: String,
    },

    /// Initialise .ai/ workspace in the current (or given) directory
    Init {
        #[arg(long, short, default_value = ".")]
        repo: String,
        #[arg(long)]
        model: Option<String>,
        #[arg(long)]
        backend: Option<String>,
        #[arg(long)]
        llm_url: Option<String>,
        #[arg(long)]
        api_key: Option<String>,
        #[arg(long, short)]
        yes: bool,
    },
}

pub async fn run() -> anyhow::Result<()> {
    let cli = Cli::parse();

    if cli.requires_global_config_init() {
        crate::config_loader::ensure_global_config();
    }

    execute_command(cli).await
}

pub async fn execute_command(cli: Cli) -> anyhow::Result<()> {
    match cli.command {
        Commands::Run {
            task,
            repo,
            config,
            role,
            system_prompt,
            max_steps,
        } => {
            set_console_logs_enabled(false);

            let repo_path = Path::new(&repo);
            let LoadedConfig {
                config: mut cfg,
                source: config_source,
            } = run_support::load_run_config(&config, repo_path);

            setup_logging(&cfg, Some(&repo))?;
            ensure_project_prompts(repo_path)?;

            cfg.validate()?;
            cfg.validate_runtime().await?;
            check_dependencies().await?;

            // ── Inbox poller ───────────────────────────────────────────────
            // Start the Telegram /inbox poller if credentials are configured.
            // The handle is stopped after agent.run() completes so the poller
            // is active for the full duration of the session and no longer.
            let inbox_poller =
                match (cfg.telegram_token.clone(), cfg.telegram_chat_id.clone()) {
                    (Some(token), Some(chat_id)) => {
                        let repo_root = repo_path.to_path_buf();
                        Some(crate::tools::start_inbox_poller(token, chat_id, repo_root))
                    }
                    _ => None,
                };

            let agent_role = resolve_role(role.as_deref())?;
            let (task_image, task_text, task_source) = run_support::resolve_run_task_input(&task)?;

            if let Some(sp) = system_prompt {
                cfg.system_prompt = load_text_or_inline(&sp, "--system-prompt")?;
                tracing::info!("System prompt: CLI override");
            }

            tracing::info!(
                "Model: {} | Repo: {} | Role: {}",
                cfg.model,
                repo,
                agent_role.name()
            );
            let mut agent = SweAgent::new(cfg, &repo, max_steps as usize, agent_role)?;
            agent.set_config_source(config_source);
            let run_result = agent.run(&task_text, task_image, task_source).await;
            set_console_logs_enabled(false);

            // Stop the inbox poller gracefully before cleanup
            if let Some(handle) = inbox_poller {
                handle.stop().await;
            }

            run_result?;

            crate::tools::cleanup_background_processes(repo_path)?;
            crate::tools::cleanup_old_logs(repo_path, 7)?;
        }

        Commands::Config { config } => {
            set_console_logs_enabled(true);
            let loaded = AgentConfig::load_or_default_with_source(&config);
            let cfg = loaded.config.clone();
            setup_logging(&cfg, None)?;
            print!("{}", run_support::format_config_output(&loaded)?);
        }

        Commands::Roles => {
            set_console_logs_enabled(true);
            setup_logging(&AgentConfig::default(), None)?;
            print_roles();
        }

        Commands::Status { repo } => {
            set_console_logs_enabled(true);
            setup_logging(&AgentConfig::default(), None)?;
            cmd_status(&repo)?;
        }

        Commands::Check { repo, config } => {
            set_console_logs_enabled(true);
            cmd_check(&repo, &config).await?;
        }

        Commands::Init {
            repo,
            model,
            backend,
            llm_url,
            api_key,
            yes,
        } => {
            set_console_logs_enabled(true);
            setup_logging(&AgentConfig::default(), None)?;
            cmd_init(
                &repo,
                model.as_deref(),
                backend.as_deref(),
                llm_url.as_deref(),
                api_key.as_deref(),
                yes,
            )?;
        }
    }
    Ok(())
}
