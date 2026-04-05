use crate::config_struct::{AI_DIR, AgentConfig, PROMPTS_DIR, Role, builtin_role_prompts};
use std::path::Path;
use tracing_subscriber::EnvFilter;

const IMAGE_EXTS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "bmp"];

pub fn resolve_role(role: Option<&str>) -> anyhow::Result<Role> {
    match role {
        None => Ok(Role::Default),
        Some(r) => Role::role_from_str(r).ok_or_else(|| {
            eprintln!(
                "Unknown role '{}'. Run `do_it roles` to see available roles.",
                r
            );
            anyhow::anyhow!("Unknown role: {r}")
        }),
    }
}

pub fn print_roles() {
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

pub fn is_image_path(value: &str) -> bool {
    let p = Path::new(value);
    if !p.exists() {
        return false;
    }
    p.extension()
        .and_then(|e| e.to_str())
        .map(|e| IMAGE_EXTS.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

pub fn load_text_or_inline(value: &str, arg_name: &str) -> anyhow::Result<String> {
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

pub fn load_task_text_or_inline(
    value: &str,
    arg_name: &str,
) -> anyhow::Result<(String, Option<String>)> {
    let p = Path::new(value);
    if p.exists() && p.is_file() && !is_image_path(value) {
        let content = std::fs::read_to_string(p)
            .map_err(|e| anyhow::anyhow!("{arg_name}: failed to read '{}': {e}", p.display()))?;
        tracing::info!("{arg_name}: loaded from file '{}'", p.display());
        Ok((
            content.trim_end().to_string(),
            Some(p.display().to_string()),
        ))
    } else {
        Ok((value.to_string(), None))
    }
}

pub fn ensure_project_prompts(root: &Path) -> anyhow::Result<()> {
    let prompts_dir = root.join(AI_DIR).join(PROMPTS_DIR);
    std::fs::create_dir_all(&prompts_dir)
        .map_err(|e| anyhow::anyhow!("Cannot create {}: {e}", prompts_dir.display()))?;

    for (name, content) in builtin_role_prompts() {
        let path = prompts_dir.join(name);
        let should_write = match std::fs::read_to_string(&path) {
            Ok(existing) => existing.trim().is_empty(),
            Err(_) => true,
        };
        if should_write {
            std::fs::write(&path, content)
                .map_err(|e| anyhow::anyhow!("Cannot write {}: {e}", path.display()))?;
        }
    }

    Ok(())
}

pub fn setup_logging(cfg: &AgentConfig, repo: Option<&str>) -> anyhow::Result<()> {
    use tracing_appender::rolling::daily;
    use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt};

    #[derive(Clone)]
    struct TuiAwareStdout;

    impl<'a> fmt::MakeWriter<'a> for TuiAwareStdout {
        type Writer = Box<dyn std::io::Write + 'a>;

        fn make_writer(&'a self) -> Self::Writer {
            if crate::tui::tui_is_active() || !super::console_logs_enabled() {
                Box::new(std::io::sink())
            } else {
                Box::new(std::io::stdout())
            }
        }
    }

    let log_level = match cfg.log_level.as_str() {
        "trace" => tracing::Level::TRACE,
        "debug" => tracing::Level::DEBUG,
        "info" => tracing::Level::INFO,
        "warn" => tracing::Level::WARN,
        "error" => tracing::Level::ERROR,
        _ => tracing::Level::INFO,
    };

    let filter = EnvFilter::builder()
        .with_default_directive(log_level.into())
        .from_env_lossy();

    if cfg.log_format == "json" {
        let stdout_layer = fmt::layer().json().with_writer(TuiAwareStdout);
        let registry = tracing_subscriber::registry()
            .with(filter)
            .with(stdout_layer);

        if let Some(repo_path) = repo {
            let logs_dir = Path::new(repo_path).join(".ai").join("logs");
            std::fs::create_dir_all(&logs_dir)?;
            let file_appender = daily(&logs_dir, "do_it.log");
            let file_layer = fmt::layer().json().with_writer(file_appender);
            registry.with(file_layer).init();
        } else {
            registry.init();
        }
    } else {
        let stdout_layer = fmt::layer()
            .with_ansi(false)
            .without_time()
            .with_writer(TuiAwareStdout);
        let registry = tracing_subscriber::registry()
            .with(filter)
            .with(stdout_layer);

        if let Some(repo_path) = repo {
            let logs_dir = Path::new(repo_path).join(".ai").join("logs");
            std::fs::create_dir_all(&logs_dir)?;
            let file_appender = daily(&logs_dir, "do_it.log");
            let file_layer = fmt::layer()
                .with_ansi(false)
                .without_time()
                .with_writer(file_appender);
            registry.with(file_layer).init();
        } else {
            registry.init();
        }
    }

    Ok(())
}

pub async fn check_dependencies() -> anyhow::Result<()> {
    let dependencies = [
        (
            "git",
            "git --version",
            "Git operations (git_status, git_commit, etc.)",
        ),
        (
            "curl",
            "curl --version",
            "HTTP fallback for web_search and fetch_url",
        ),
    ];

    for (name, command, purpose) in &dependencies {
        let output = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            tokio::process::Command::new("cmd")
                .args(["/C", command])
                .output(),
        )
        .await;

        match output {
            Ok(Ok(out)) if out.status.success() => {
                tracing::debug!("Dependency '{}' found", name);
            }
            _ => {
                tracing::warn!(
                    "Dependency '{}' not found — {} will not work. Install {} to enable.",
                    name,
                    purpose,
                    name
                );
            }
        }
    }

    Ok(())
}
