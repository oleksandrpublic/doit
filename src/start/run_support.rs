use crate::config_loader::LoadedConfig;
use crate::config_struct::AgentConfig;
use std::path::Path;

pub(crate) fn load_run_config(config_arg: &str, repo_path: &Path) -> LoadedConfig {
    let explicit_config = if config_arg != "config.toml" {
        Some(Path::new(config_arg))
    } else {
        None
    };

    AgentConfig::load_for_repo_with_source(explicit_config, Some(repo_path))
}

pub(crate) fn format_config_output(loaded: &LoadedConfig) -> anyhow::Result<String> {
    Ok(format!(
        "# source: {}\n{}",
        loaded.source,
        toml::to_string_pretty(&loaded.config)?
    ))
}

pub(crate) fn resolve_run_task_input(
    task: &str,
) -> anyhow::Result<(Option<std::path::PathBuf>, String, Option<String>)> {
    let task_image = if super::is_image_path(task) {
        tracing::info!("--task: image detected '{}'", task);
        Some(std::path::PathBuf::from(task))
    } else {
        None
    };

    let (task_text, task_source) = if task_image.is_some() {
        (String::new(), None)
    } else {
        super::load_task_text_or_inline(task, "--task")?
    };

    Ok((task_image, task_text, task_source))
}
