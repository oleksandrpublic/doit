use super::core::{ToolResult, resolve, str_arg};
use crate::validation::validate_program;
use anyhow::{Result, anyhow};
use serde_json::Value;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use tokio::process::Command;

const RUN_COMMAND_DEFAULT_TIMEOUT_SECS: u64 = 90;
const RUN_COMMAND_MAX_TIMEOUT_SECS: u64 = 300;
const RUN_COMMAND_MAX_ARGS: usize = 128;
const RUN_COMMAND_MAX_ARG_LEN: usize = 4096;
const RUN_COMMAND_MAX_ENV_VARS: usize = 32;
const RUN_COMMAND_MAX_ENV_VALUE_LEN: usize = 8192;
const BLOCKED_ENV_KEYS: &[&str] = &[
    "PATH",
    "PATHEXT",
    "COMSPEC",
    "LD_PRELOAD",
    "DYLD_INSERT_LIBRARIES",
    "RUSTC_WRAPPER",
    "CARGO_HOME",
    "RUSTUP_HOME",
    "HOME",
    "USERPROFILE",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProjectKind {
    Rust,
    Node,
    Python,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TargetedTestKind {
    Auto,
    Lib,
    Test,
    Package,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TargetedTestCommand {
    program: String,
    args: Vec<String>,
    cwd: PathBuf,
    description: String,
}

/// Execute a program with arguments (no shell).
/// Args:
///   program     — executable name or path to run
///   args        — array of arguments to pass to the program
///   cwd?        — working directory (default: repo root)
///   timeout_secs? — kill after this many seconds (default: 30)
///   env?        — object with environment variables to set
///
/// Security: program is executed directly without a shell.
/// If allowlist is non-empty, program must be in the allowlist.
pub async fn run_command(args: &Value, root: &Path, allowlist: &[String]) -> Result<ToolResult> {
    let program = str_arg(args, "program")?;
    validate_run_command_program(&program)?;

    if !allowlist.is_empty() && !allowlist.contains(&program) {
        return Err(anyhow!(
            "run_command: program '{}' is not in command_allowlist {:?}",
            program,
            allowlist
        ));
    }
    validate_program(&program)?;

    let args_arr = args
        .get("args")
        .and_then(|v| v.as_array())
        .ok_or_else(|| anyhow!("Missing arg: args"))?;
    if args_arr.len() > RUN_COMMAND_MAX_ARGS {
        return Err(anyhow!(
            "run_command: too many args ({} > max {})",
            args_arr.len(),
            RUN_COMMAND_MAX_ARGS
        ));
    }

    let cwd = if let Some(p) = args.get("cwd").and_then(|v| v.as_str()) {
        resolve(root, p)?
    } else {
        root.to_path_buf()
    };

    let timeout_secs = sanitize_run_command_timeout(
        args.get("timeout_secs")
            .and_then(|v| v.as_u64())
            .unwrap_or(RUN_COMMAND_DEFAULT_TIMEOUT_SECS),
    )?;
    let env = parse_run_command_env(args.get("env"))?;

    let command_args = args_arr
        .iter()
        .map(|arg| {
            let value = arg
                .as_str()
                .ok_or_else(|| anyhow!("All args entries must be strings"))?;
            if value.len() > RUN_COMMAND_MAX_ARG_LEN {
                return Err(anyhow!(
                    "run_command: arg too long ({} > max {})",
                    value.len(),
                    RUN_COMMAND_MAX_ARG_LEN
                ));
            }
            Ok(value.to_string())
        })
        .collect::<Result<Vec<_>>>()?;

    execute_command(
        &program,
        &command_args,
        &cwd,
        &env,
        timeout_secs,
        "run_command",
    )
    .await
}

/// Run a focused test command, preferring a narrow cargo test target over the full suite.
/// Rust-first for now.
///
/// Args:
///   path?         — file path to infer a likely test target (e.g. tests/integration/main.rs)
///   test?         — optional test filter/name
///   kind?         — "auto" (default), "lib", "test", "package"
///   target?       — explicit cargo test target name when kind="test"
///   dir?/cwd?     — working directory (default: repo root)
///   timeout_secs? — timeout (default: 120)
pub async fn run_targeted_test(args: &Value, root: &Path) -> Result<ToolResult> {
    let cwd = resolve_cwd(args, root)?;
    let timeout_secs = args
        .get("timeout_secs")
        .and_then(|v| v.as_u64())
        .unwrap_or(120);
    let project_kind = detect_project_kind(&cwd).ok_or_else(|| {
        anyhow!(
            "run_targeted_test: could not detect project type in {}",
            cwd.display()
        )
    })?;

    if project_kind != ProjectKind::Rust {
        return Ok(ToolResult::failure(format!(
            "run_targeted_test currently supports Rust projects only. Detected {} in {}",
            project_kind.label(),
            cwd.display()
        )));
    }

    let command = build_rust_targeted_test_command(args, &cwd)?;
    let mut result = execute_command(
        &command.program,
        &command.args,
        &command.cwd,
        &[],
        timeout_secs,
        "run_targeted_test",
    )
    .await?;

    result.output = format!(
        "Targeted test command: {}\n\n{}",
        command.description, result.output
    );
    Ok(result)
}

/// Format only changed files in the current git repository.
/// Rust-first for now.
///
/// Args:
///   dir?/cwd?     — working directory (default: repo root)
///   check_only?   — if true, use rustfmt --check instead of rewriting files
///   timeout_secs? — timeout (default: 120)
pub async fn format_changed_files_only(args: &Value, root: &Path) -> Result<ToolResult> {
    let cwd = resolve_cwd(args, root)?;
    let timeout_secs = args
        .get("timeout_secs")
        .and_then(|v| v.as_u64())
        .unwrap_or(120);
    let check_only = args
        .get("check_only")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let project_kind = detect_project_kind(&cwd).ok_or_else(|| {
        anyhow!(
            "format_changed_files_only: could not detect project type in {}",
            cwd.display()
        )
    })?;

    if project_kind != ProjectKind::Rust {
        return Ok(ToolResult::failure(format!(
            "format_changed_files_only currently supports Rust projects only. Detected {} in {}",
            project_kind.label(),
            cwd.display()
        )));
    }

    let changed_files = collect_changed_git_paths(&cwd, timeout_secs).await?;
    let rust_files = changed_files
        .into_iter()
        .filter(|path| path.ends_with(".rs"))
        .filter(|path| cwd.join(path).is_file())
        .collect::<Vec<_>>();

    if rust_files.is_empty() {
        return Ok(ToolResult::ok("No changed Rust files to format"));
    }

    let mut formatter_args = Vec::new();
    if check_only {
        formatter_args.push("--check".to_string());
    }
    formatter_args.extend(rust_files.iter().cloned());

    let mut result = execute_command(
        "rustfmt",
        &formatter_args,
        &cwd,
        &[],
        timeout_secs,
        "format_changed_files_only",
    )
    .await?;

    let mode = if check_only { "Checked" } else { "Formatted" };
    let file_list = rust_files
        .iter()
        .map(|path| format!("- {path}"))
        .collect::<Vec<_>>()
        .join("\n");

    result.output = if result.output.trim().is_empty() {
        format!(
            "{mode} {} changed Rust file(s):\n{file_list}",
            rust_files.len()
        )
    } else {
        format!(
            "{mode} {} changed Rust file(s):\n{file_list}\n\n{}",
            rust_files.len(),
            result.output
        )
    };

    Ok(result)
}

async fn execute_command(
    program: &str,
    args: &[String],
    cwd: &Path,
    env: &[(String, String)],
    timeout_secs: u64,
    label: &str,
) -> Result<ToolResult> {
    let mut command = Command::new(program);
    command.args(args);
    command.current_dir(cwd);
    for (key, value) in env {
        command.env(key, value);
    }

    let output = tokio::time::timeout(
        std::time::Duration::from_secs(timeout_secs),
        command.output(),
    )
    .await;

    match output {
        Ok(Ok(out)) => {
            let stdout = String::from_utf8_lossy(&out.stdout).to_string();
            let stderr = String::from_utf8_lossy(&out.stderr).to_string();
            let success = out.status.success();

            // Always include non-empty stderr — cargo/rustfmt write warnings there even on success.
            let output_text = match (stdout.trim().is_empty(), stderr.trim().is_empty()) {
                (false, false) => format!("{}\n\nstderr:\n{}", stdout.trim(), stderr.trim()),
                (true, false) => format!("stderr:\n{}", stderr.trim()),
                (false, true) => stdout,
                (true, true) => String::new(),
            };

            Ok(ToolResult {
                output: output_text,
                success,
            })
        }
        Ok(Err(e)) => Ok(ToolResult {
            output: format!("{label}: failed to spawn process: {e}"),
            success: false,
        }),
        Err(_) => Ok(ToolResult {
            output: format!("{label}: timeout after {timeout_secs}s (killed)"),
            success: false,
        }),
    }
}

fn resolve_cwd(args: &Value, root: &Path) -> Result<PathBuf> {
    if let Some(p) = args
        .get("cwd")
        .and_then(|v| v.as_str())
        .or_else(|| args.get("dir").and_then(|v| v.as_str()))
    {
        resolve(root, p)
    } else {
        Ok(root.to_path_buf())
    }
}

fn validate_run_command_program(program: &str) -> Result<()> {
    if program.trim() != program || program.is_empty() {
        return Err(anyhow!(
            "run_command: program cannot be empty or padded with whitespace"
        ));
    }
    if program.contains('/') || program.contains('\\') {
        return Err(anyhow!(
            "run_command: program must be a bare executable name from PATH, not a path"
        ));
    }
    Ok(())
}

fn sanitize_run_command_timeout(timeout_secs: u64) -> Result<u64> {
    if timeout_secs == 0 {
        return Err(anyhow!("run_command: timeout_secs must be at least 1"));
    }
    if timeout_secs > RUN_COMMAND_MAX_TIMEOUT_SECS {
        return Err(anyhow!(
            "run_command: timeout_secs {} exceeds max {}",
            timeout_secs,
            RUN_COMMAND_MAX_TIMEOUT_SECS
        ));
    }
    Ok(timeout_secs)
}

fn parse_run_command_env(env_value: Option<&Value>) -> Result<Vec<(String, String)>> {
    let Some(env_obj) = env_value else {
        return Ok(Vec::new());
    };
    let obj = env_obj
        .as_object()
        .ok_or_else(|| anyhow!("run_command: env must be an object"))?;
    if obj.len() > RUN_COMMAND_MAX_ENV_VARS {
        return Err(anyhow!(
            "run_command: too many env vars ({} > max {})",
            obj.len(),
            RUN_COMMAND_MAX_ENV_VARS
        ));
    }

    let mut env = Vec::with_capacity(obj.len());
    for (key, value) in obj {
        validate_run_command_env_key(key)?;
        let string_value = value
            .as_str()
            .ok_or_else(|| anyhow!("run_command: env values must be strings"))?;
        if string_value.len() > RUN_COMMAND_MAX_ENV_VALUE_LEN {
            return Err(anyhow!(
                "run_command: env value for '{}' is too long ({} > max {})",
                key,
                string_value.len(),
                RUN_COMMAND_MAX_ENV_VALUE_LEN
            ));
        }
        env.push((key.clone(), string_value.to_string()));
    }
    Ok(env)
}

fn validate_run_command_env_key(key: &str) -> Result<()> {
    if key.is_empty() {
        return Err(anyhow!("run_command: env var names cannot be empty"));
    }
    let mut chars = key.chars();
    let first = chars.next().unwrap();
    if !(first.is_ascii_alphabetic() || first == '_') {
        return Err(anyhow!(
            "run_command: env var '{}' must start with an ASCII letter or '_'",
            key
        ));
    }
    if !chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_') {
        return Err(anyhow!(
            "run_command: env var '{}' contains unsupported characters",
            key
        ));
    }
    if BLOCKED_ENV_KEYS
        .iter()
        .any(|blocked| blocked.eq_ignore_ascii_case(key))
    {
        return Err(anyhow!(
            "run_command: env var '{}' is blocked by command policy",
            key
        ));
    }
    Ok(())
}

fn detect_project_kind(root: &Path) -> Option<ProjectKind> {
    if root.join("Cargo.toml").exists() {
        return Some(ProjectKind::Rust);
    }
    if root.join("package.json").exists() {
        return Some(ProjectKind::Node);
    }
    if root.join("pyproject.toml").exists()
        || root.join("pytest.ini").exists()
        || root.join("setup.py").exists()
    {
        return Some(ProjectKind::Python);
    }
    None
}

impl ProjectKind {
    fn label(self) -> &'static str {
        match self {
            Self::Rust => "Rust",
            Self::Node => "Node",
            Self::Python => "Python",
        }
    }
}

fn build_rust_targeted_test_command(args: &Value, cwd: &Path) -> Result<TargetedTestCommand> {
    let kind = parse_targeted_test_kind(args.get("kind").and_then(|v| v.as_str()))?;
    let test_filter = args
        .get("test")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let explicit_target = args
        .get("target")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let normalized_path = args
        .get("path")
        .and_then(|v| v.as_str())
        .map(normalize_path_for_matching);

    let mut cargo_args = vec!["test".to_string()];
    let mut description = String::new();

    match kind {
        TargetedTestKind::Lib => {
            cargo_args.push("--lib".to_string());
            description.push_str("cargo test --lib");
        }
        TargetedTestKind::Test => {
            let target = explicit_target
                .or_else(|| {
                    normalized_path
                        .as_deref()
                        .and_then(infer_test_target_from_path)
                })
                .ok_or_else(|| {
                    anyhow!("run_targeted_test: kind='test' requires target or a path under tests/")
                })?;
            cargo_args.push("--test".to_string());
            cargo_args.push(target.clone());
            description.push_str(&format!("cargo test --test {target}"));
        }
        TargetedTestKind::Package => {
            description.push_str("cargo test");
        }
        TargetedTestKind::Auto => {
            if let Some(path) = normalized_path.as_deref() {
                if let Some(target) = infer_test_target_from_path(path) {
                    cargo_args.push("--test".to_string());
                    cargo_args.push(target.clone());
                    description.push_str(&format!("cargo test --test {target}"));
                } else if path.starts_with("src/") || path == "src" {
                    cargo_args.push("--lib".to_string());
                    description.push_str("cargo test --lib");
                } else {
                    description.push_str("cargo test");
                }
            } else {
                description.push_str("cargo test");
            }
        }
    }

    if let Some(filter) = test_filter {
        cargo_args.push(filter.clone());
        description.push(' ');
        description.push_str(&filter);
    }

    Ok(TargetedTestCommand {
        program: "cargo".to_string(),
        args: cargo_args,
        cwd: cwd.to_path_buf(),
        description,
    })
}

fn parse_targeted_test_kind(kind: Option<&str>) -> Result<TargetedTestKind> {
    match kind.unwrap_or("auto") {
        "auto" => Ok(TargetedTestKind::Auto),
        "lib" => Ok(TargetedTestKind::Lib),
        "test" => Ok(TargetedTestKind::Test),
        "package" => Ok(TargetedTestKind::Package),
        other => Err(anyhow!(
            "run_targeted_test: unknown kind '{other}'. Valid: auto, lib, test, package"
        )),
    }
}

fn normalize_path_for_matching(path: &str) -> String {
    let mut normalized = path.replace('\\', "/");
    while let Some(stripped) = normalized.strip_prefix("./") {
        normalized = stripped.to_string();
    }
    while let Some(stripped) = normalized.strip_prefix('/') {
        normalized = stripped.to_string();
    }
    normalized
}

fn infer_test_target_from_path(path: &str) -> Option<String> {
    let normalized = normalize_path_for_matching(path);
    let rest = normalized.strip_prefix("tests/")?;
    let stem = rest.strip_suffix(".rs")?;
    let parts = stem.split('/').collect::<Vec<_>>();
    if parts.is_empty() {
        None
    } else {
        Some(parts[0].to_string())
    }
}

async fn collect_changed_git_paths(cwd: &Path, timeout_secs: u64) -> Result<Vec<String>> {
    let mut cmd = Command::new("git");
    cmd.arg("status").arg("--porcelain").current_dir(cwd);

    let output =
        tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), cmd.output()).await;

    match output {
        Ok(Ok(out)) => {
            let stdout = String::from_utf8_lossy(&out.stdout).to_string();
            let stderr = String::from_utf8_lossy(&out.stderr).to_string();
            if out.status.success() {
                Ok(parse_changed_paths_from_git_status(&stdout))
            } else {
                Err(anyhow!("git status failed: {}", stderr.trim()))
            }
        }
        Ok(Err(e)) => Err(anyhow!("git status failed to spawn: {e}")),
        Err(_) => Err(anyhow!("git status timed out after {timeout_secs}s")),
    }
}

fn parse_changed_paths_from_git_status(stdout: &str) -> Vec<String> {
    let mut out = BTreeSet::new();

    for line in stdout.lines() {
        let mut chars = line.chars();
        let first = match chars.next() {
            Some(ch) => ch,
            None => continue,
        };
        let second = match chars.next() {
            Some(ch) => ch,
            None => continue,
        };
        if chars.next() != Some(' ') {
            continue;
        }

        let status = [first, second];
        if status.contains(&'D') {
            continue;
        }

        let raw_path = chars.as_str().trim();
        if raw_path.is_empty() {
            continue;
        }

        let path = if let Some((_, new_path)) = raw_path.split_once(" -> ") {
            new_path.trim()
        } else {
            raw_path
        };

        let normalized = normalize_path_for_matching(path);
        if !normalized.is_empty() {
            out.insert(normalized);
        }
    }

    out.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn test_run_command_echo() {
        let args = json!({
            "program": "cmd",
            "args": ["/C", "echo", "hello"]
        });
        let root = PathBuf::from(".");
        let result = run_command(&args, &root, &[]).await.unwrap();
        assert!(result.success);
        assert!(result.output.contains("hello"));
    }

    #[tokio::test]
    async fn test_run_command_with_cwd() {
        let args = json!({
            "program": "cmd",
            "args": ["/C", "cd"],
            "cwd": "."
        });
        let root = PathBuf::from(".").canonicalize().unwrap();
        let result = run_command(&args, &root, &[]).await.unwrap();
        assert!(result.success);
    }

    #[tokio::test]
    async fn test_run_command_with_env() {
        let args = json!({
            "program": "cmd",
            "args": ["/C", "echo", "%TEST_VAR%"],
            "env": { "TEST_VAR": "test_value" }
        });
        let root = PathBuf::from(".");
        let result = run_command(&args, &root, &[]).await.unwrap();
        assert!(result.success);
        assert!(result.output.contains("test_value"));
    }

    #[tokio::test]
    async fn test_run_command_timeout() {
        let args = json!({
            "program": "ping",
            "args": ["-n", "10", "127.0.0.1"],
            "timeout_secs": 1
        });
        let root = PathBuf::from(".");
        let result = run_command(&args, &root, &[]).await.unwrap();
        assert!(!result.success);
        assert!(result.output.contains("timeout"));
    }

    #[tokio::test]
    async fn test_run_command_failing_command() {
        let args = json!({
            "program": "cmd",
            "args": ["/C", "exit", "1"]
        });
        let root = PathBuf::from(".");
        let result = run_command(&args, &root, &[]).await.unwrap();
        assert!(!result.success);
    }

    #[tokio::test]
    async fn test_run_command_missing_program() {
        let args = json!({
            "args": ["echo", "test"]
        });
        let root = PathBuf::from(".");
        let result = run_command(&args, &root, &[]).await;
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Missing arg: program")
        );
    }

    #[tokio::test]
    async fn test_run_command_missing_args() {
        let args = json!({
            "program": "echo"
        });
        let root = PathBuf::from(".");
        let result = run_command(&args, &root, &[]).await;
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Missing arg: args")
        );
    }

    #[tokio::test]
    async fn test_run_command_invalid_cwd() {
        let args = json!({
            "program": "cmd",
            "args": ["/C", "echo", "test"],
            "cwd": "../outside_root"
        });
        let root = PathBuf::from(".").canonicalize().unwrap();
        let result = run_command(&args, &root, &[]).await;
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("Path traversal detected"));
    }

    #[test]
    fn run_command_rejects_program_paths() {
        let err = validate_run_command_program("./cargo")
            .unwrap_err()
            .to_string();
        assert!(err.contains("bare executable name"));
    }

    #[test]
    fn run_command_rejects_excessive_timeout() {
        let err = sanitize_run_command_timeout(RUN_COMMAND_MAX_TIMEOUT_SECS + 1)
            .unwrap_err()
            .to_string();
        assert!(err.contains("exceeds max"));
    }

    #[test]
    fn run_command_rejects_blocked_env_keys() {
        let err = validate_run_command_env_key("PATH")
            .unwrap_err()
            .to_string();
        assert!(err.contains("blocked by command policy"));
    }

    #[test]
    fn run_command_rejects_invalid_env_key_shape() {
        let err = validate_run_command_env_key("BAD-KEY")
            .unwrap_err()
            .to_string();
        assert!(err.contains("unsupported characters"));
    }

    #[test]
    fn run_command_parses_valid_env_object() {
        let env = parse_run_command_env(Some(&json!({
            "TEST_VAR": "value",
            "_SECOND": "two"
        })))
        .unwrap();
        assert_eq!(
            env,
            vec![
                ("TEST_VAR".to_string(), "value".to_string()),
                ("_SECOND".to_string(), "two".to_string()),
            ]
        );
    }

    #[test]
    fn infer_target_from_integration_path() {
        assert_eq!(
            infer_test_target_from_path("tests/integration/main.rs"),
            Some("integration".to_string())
        );
    }

    #[test]
    fn build_targeted_test_command_auto_for_src_path() {
        let cwd = PathBuf::from(".");
        let args = json!({
            "path": "src/tools/commands.rs",
            "test": "test_run_command_echo"
        });
        let cmd = build_rust_targeted_test_command(&args, &cwd).unwrap();
        assert_eq!(cmd.args, vec!["test", "--lib", "test_run_command_echo"]);
    }

    #[test]
    fn build_targeted_test_command_auto_for_tests_path() {
        let cwd = PathBuf::from(".");
        let args = json!({
            "path": "tests/integration/main.rs",
            "test": "commands_tool::test_run_command_echo"
        });
        let cmd = build_rust_targeted_test_command(&args, &cwd).unwrap();
        assert_eq!(
            cmd.args,
            vec![
                "test",
                "--test",
                "integration",
                "commands_tool::test_run_command_echo"
            ]
        );
    }

    #[test]
    fn build_targeted_test_command_rejects_unknown_kind() {
        let cwd = PathBuf::from(".");
        let args = json!({ "kind": "weird" });
        let err = build_rust_targeted_test_command(&args, &cwd)
            .unwrap_err()
            .to_string();
        assert!(err.contains("unknown kind"));
    }

    #[test]
    fn parse_changed_paths_from_status_handles_mixed_entries() {
        let stdout = concat!(
            "MM src/tools/commands.rs\n",
            "?? src/new_tool.rs\n",
            "R  src/old_name.rs -> src/new_name.rs\n",
            "D  src/deleted.rs\n",
        );
        let paths = parse_changed_paths_from_git_status(stdout);
        assert_eq!(
            paths,
            vec![
                "src/new_name.rs".to_string(),
                "src/new_tool.rs".to_string(),
                "src/tools/commands.rs".to_string(),
            ]
        );
    }

    #[test]
    fn parse_changed_paths_from_status_deduplicates_and_normalizes() {
        let stdout = "\
 M .\\src\\lib.rs\n\
AM src/lib.rs\n";
        let paths = parse_changed_paths_from_git_status(stdout);
        assert_eq!(paths, vec!["src/lib.rs".to_string()]);
    }
}
