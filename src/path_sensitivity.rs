use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathSensitivity {
    OutsideWorkspace,
    RepoMeta,
    ProjectConfig,
    RuntimeState,
    Prompts,
    Knowledge,
    Memory,
    Source,
}

impl PathSensitivity {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::OutsideWorkspace => "outside_workspace",
            Self::RepoMeta => "repo_meta",
            Self::ProjectConfig => "project_config",
            Self::RuntimeState => "runtime_state",
            Self::Prompts => "prompts",
            Self::Knowledge => "knowledge",
            Self::Memory => "memory",
            Self::Source => "source",
        }
    }

    pub fn outcome_tag(self) -> String {
        format!("[sensitivity: {}]", self.as_str())
    }

    pub fn soft_write_warning(self) -> Option<&'static str> {
        match self {
            Self::RepoMeta => Some("editing repo metadata can affect git behavior"),
            Self::ProjectConfig => Some("editing project config can affect future runs"),
            Self::Prompts => Some("editing prompts can change future agent behavior"),
            _ => None,
        }
    }

    pub fn policy_note(self) -> Option<&'static str> {
        match self {
            Self::RepoMeta => Some("review git diff/status carefully after this edit"),
            Self::ProjectConfig => Some("re-check resolved config or runtime behavior after this edit"),
            Self::Prompts => Some("expect future agent sessions to follow the updated prompt"),
            _ => None,
        }
    }

    pub fn outcome_annotations(self) -> String {
        let mut parts = vec![self.outcome_tag()];
        if let Some(warning) = self.soft_write_warning() {
            parts.push(format!("[warning: {warning}]"));
        }
        if let Some(policy) = self.policy_note() {
            parts.push(format!("[policy: {policy}]"));
        }
        parts.join(" ")
    }
}

pub fn classify_path_sensitivity(root: &Path, path: &Path) -> PathSensitivity {
    let root = normalize_for_classification(root);
    let candidate = if path.is_absolute() {
        normalize_for_classification(path)
    } else {
        normalize_for_classification(&root.join(path))
    };

    let Ok(relative) = candidate.strip_prefix(&root) else {
        return PathSensitivity::OutsideWorkspace;
    };

    let mut parts = relative.components().filter_map(component_to_str);
    let first = match parts.next() {
        Some(first) => first,
        None => return PathSensitivity::Source,
    };

    match first {
        ".git" => PathSensitivity::RepoMeta,
        "config.toml" | ".gitignore" => PathSensitivity::ProjectConfig,
        ".ai" => match parts.next() {
            Some("state" | "logs" | "screenshots") => PathSensitivity::RuntimeState,
            Some("prompts") => PathSensitivity::Prompts,
            Some("knowledge") => PathSensitivity::Knowledge,
            Some("memory") => PathSensitivity::Memory,
            Some("project.toml") => PathSensitivity::ProjectConfig,
            _ => PathSensitivity::RuntimeState,
        },
        _ => PathSensitivity::Source,
    }
}

fn normalize_for_classification(path: &Path) -> PathBuf {
    let stripped = strip_verbatim_prefix(path);
    let mut components = Vec::new();

    for component in stripped.components() {
        match component {
            Component::ParentDir => {
                if matches!(components.last(), Some(Component::Normal(_))) {
                    components.pop();
                } else if !matches!(
                    components.last(),
                    Some(Component::RootDir | Component::Prefix(_))
                ) {
                    components.push(component);
                }
            }
            Component::CurDir => {}
            _ => components.push(component),
        }
    }

    components.iter().collect()
}

fn strip_verbatim_prefix(path: &Path) -> PathBuf {
    let rendered = path.to_string_lossy();
    if let Some(stripped) = rendered.strip_prefix("\\\\?\\") {
        PathBuf::from(stripped)
    } else {
        path.to_path_buf()
    }
}

fn component_to_str(component: Component<'_>) -> Option<&str> {
    match component {
        Component::Normal(value) => value.to_str(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn classify_path_sensitivity_marks_repo_source_files() {
        let temp = TempDir::new().unwrap();

        assert_eq!(
            classify_path_sensitivity(temp.path(), Path::new("src/lib.rs")),
            PathSensitivity::Source
        );
    }

    #[test]
    fn classify_path_sensitivity_marks_project_config_files() {
        let temp = TempDir::new().unwrap();

        assert_eq!(
            classify_path_sensitivity(temp.path(), Path::new("config.toml")),
            PathSensitivity::ProjectConfig
        );
        assert_eq!(
            classify_path_sensitivity(temp.path(), Path::new(".ai/project.toml")),
            PathSensitivity::ProjectConfig
        );
    }

    #[test]
    fn classify_path_sensitivity_marks_runtime_state_files() {
        let temp = TempDir::new().unwrap();

        assert_eq!(
            classify_path_sensitivity(temp.path(), Path::new(".ai/state/current_plan.md")),
            PathSensitivity::RuntimeState
        );
        assert_eq!(
            classify_path_sensitivity(temp.path(), Path::new(".ai/logs/session-001.md")),
            PathSensitivity::RuntimeState
        );
    }

    #[test]
    fn classify_path_sensitivity_marks_prompt_knowledge_and_memory_files() {
        let temp = TempDir::new().unwrap();

        assert_eq!(
            classify_path_sensitivity(temp.path(), Path::new(".ai/prompts/boss.md")),
            PathSensitivity::Prompts
        );
        assert_eq!(
            classify_path_sensitivity(temp.path(), Path::new(".ai/knowledge/decision.md")),
            PathSensitivity::Knowledge
        );
        assert_eq!(
            classify_path_sensitivity(temp.path(), Path::new(".ai/memory/user_profile.txt")),
            PathSensitivity::Memory
        );
    }

    #[test]
    fn classify_path_sensitivity_marks_git_metadata() {
        let temp = TempDir::new().unwrap();

        assert_eq!(
            classify_path_sensitivity(temp.path(), Path::new(".git/config")),
            PathSensitivity::RepoMeta
        );
    }

    #[test]
    fn classify_path_sensitivity_marks_outside_workspace_paths() {
        let temp = TempDir::new().unwrap();
        let outside = temp.path().parent().unwrap().join("outside.txt");

        assert_eq!(
            classify_path_sensitivity(temp.path(), Path::new("../outside.txt")),
            PathSensitivity::OutsideWorkspace
        );
        assert_eq!(
            classify_path_sensitivity(temp.path(), &outside),
            PathSensitivity::OutsideWorkspace
        );
    }

    #[test]
    fn high_sensitivity_categories_expose_soft_write_warnings() {
        assert_eq!(
            PathSensitivity::ProjectConfig.soft_write_warning(),
            Some("editing project config can affect future runs")
        );
        assert_eq!(
            PathSensitivity::RepoMeta.soft_write_warning(),
            Some("editing repo metadata can affect git behavior")
        );
        assert_eq!(
            PathSensitivity::Prompts.soft_write_warning(),
            Some("editing prompts can change future agent behavior")
        );
        assert_eq!(PathSensitivity::Source.soft_write_warning(), None);
    }

    #[test]
    fn high_sensitivity_categories_expose_policy_notes() {
        assert_eq!(
            PathSensitivity::ProjectConfig.policy_note(),
            Some("re-check resolved config or runtime behavior after this edit")
        );
        assert_eq!(
            PathSensitivity::RepoMeta.policy_note(),
            Some("review git diff/status carefully after this edit")
        );
        assert_eq!(
            PathSensitivity::Prompts.policy_note(),
            Some("expect future agent sessions to follow the updated prompt")
        );
        assert_eq!(PathSensitivity::Source.policy_note(), None);
    }
}
