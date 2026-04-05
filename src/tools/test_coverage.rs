use crate::tools::core::resolve;
use crate::tools::tool_result::ToolResult;
use anyhow::{anyhow, Result};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::process::Command;

const DEFAULT_THRESHOLD: f64 = 80.0;
const DEFAULT_TIMEOUT_SECS: u64 = 300;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProjectKind {
    Rust,
    Node,
    Python,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CoverageBackend {
    LlvmCov,
    Tarpaulin,
    TestFallback,
}

impl CoverageBackend {
    fn label(self) -> &'static str {
        match self {
            Self::LlvmCov => "cargo llvm-cov",
            Self::Tarpaulin => "cargo tarpaulin",
            Self::TestFallback => "cargo test (fallback)",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
struct CoverageReport {
    coverage_pct: Option<f64>,
    test_summary: Option<String>,
    raw_excerpt: String,
    command: String,
    backend: CoverageBackend,
}

pub async fn test_coverage(args: &Value, root: &Path) -> Result<ToolResult> {
    let workdir = resolve_workdir(args, root)?;
    let threshold = args
        .get("threshold")
        .and_then(|v| v.as_f64())
        .unwrap_or(DEFAULT_THRESHOLD);
    let timeout_secs = args
        .get("timeout_secs")
        .and_then(|v| v.as_u64())
        .unwrap_or(DEFAULT_TIMEOUT_SECS);

    let project_kind = detect_project_kind(&workdir).ok_or_else(|| {
        anyhow!(
            "test_coverage: could not detect project type in {}",
            workdir.display()
        )
    })?;

    match project_kind {
        ProjectKind::Rust => run_rust_coverage(&workdir, threshold, timeout_secs).await,
        ProjectKind::Node | ProjectKind::Python => Ok(ToolResult::failure(format!(
            "test_coverage: {} coverage is not implemented yet for {}\nInstall a Rust coverage backend or extend this tool for this project type.",
            project_kind.label(),
            workdir.display()
        ))),
    }
}

fn resolve_workdir(args: &Value, root: &Path) -> Result<PathBuf> {
    if let Some(dir) = args.get("dir").and_then(|v| v.as_str()) {
        resolve(root, dir)
    } else {
        Ok(root.to_path_buf())
    }
}

fn detect_project_kind(root: &Path) -> Option<ProjectKind> {
    if root.join("Cargo.toml").exists() {
        return Some(ProjectKind::Rust);
    }
    if root.join("package.json").exists() {
        return Some(ProjectKind::Node);
    }
    if root.join("pyproject.toml").exists()
        || root.join("setup.py").exists()
        || root.join("pytest.ini").exists()
    {
        return Some(ProjectKind::Python);
    }
    None
}

async fn run_rust_coverage(
    workdir: &Path,
    threshold: f64,
    timeout_secs: u64,
) -> Result<ToolResult> {
    if command_available("cargo", &["llvm-cov", "--version"]).await {
        return execute_rust_backend(workdir, threshold, timeout_secs, CoverageBackend::LlvmCov)
            .await;
    }

    if command_available("cargo", &["tarpaulin", "--version"]).await {
        return execute_rust_backend(workdir, threshold, timeout_secs, CoverageBackend::Tarpaulin)
            .await;
    }

    fallback_without_coverage(workdir, threshold, timeout_secs).await
}

async fn command_available(program: &str, args: &[&str]) -> bool {
    let mut command = Command::new(program);
    command.args(args);
    command.stdout(Stdio::null());
    command.stderr(Stdio::null());
    match command.status().await {
        Ok(status) => status.success(),
        Err(_) => false,
    }
}

async fn execute_rust_backend(
    workdir: &Path,
    threshold: f64,
    timeout_secs: u64,
    backend: CoverageBackend,
) -> Result<ToolResult> {
    let (program, args) = coverage_command_args(backend, threshold, timeout_secs);
    let command_display = format!("{program} {}", args.join(" "));
    let run = run_with_timeout(program, &args, workdir, timeout_secs + 30).await?;
    let combined = combined_output(&run.stdout, &run.stderr);
    let report = CoverageReport {
        coverage_pct: parse_coverage_pct(&combined),
        test_summary: extract_test_summary(&combined),
        raw_excerpt: excerpt(&combined, 30),
        command: command_display,
        backend,
    };

    Ok(format_report(report, threshold, workdir, run.success, None))
}

fn coverage_command_args(
    backend: CoverageBackend,
    threshold: f64,
    timeout_secs: u64,
) -> (&'static str, Vec<String>) {
    match backend {
        CoverageBackend::LlvmCov => (
            "cargo",
            vec![
                "llvm-cov".to_string(),
                "--summary-only".to_string(),
                "--locked".to_string(),
                "--offline".to_string(),
            ],
        ),
        CoverageBackend::Tarpaulin => (
            "cargo",
            vec![
                "tarpaulin".to_string(),
                "--out".to_string(),
                "Stdout".to_string(),
                "--ignore-tests".to_string(),
                "--timeout".to_string(),
                timeout_secs.to_string(),
                "--fail-under".to_string(),
                format!("{:.0}", threshold),
                "--locked".to_string(),
                "--offline".to_string(),
            ],
        ),
        CoverageBackend::TestFallback => (
            "cargo",
            vec![
                "test".to_string(),
                "--locked".to_string(),
                "--offline".to_string(),
            ],
        ),
    }
}

async fn fallback_without_coverage(
    workdir: &Path,
    threshold: f64,
    timeout_secs: u64,
) -> Result<ToolResult> {
    let args = vec![
        "test".to_string(),
        "--locked".to_string(),
        "--offline".to_string(),
    ];
    let run = run_with_timeout("cargo", &args, workdir, timeout_secs).await?;
    let combined = combined_output(&run.stdout, &run.stderr);
    let report = CoverageReport {
        coverage_pct: None,
        test_summary: extract_test_summary(&combined),
        raw_excerpt: excerpt(&combined, 30),
        command: format!("cargo {}", args.join(" ")),
        backend: CoverageBackend::TestFallback,
    };

    Ok(format_report(
        report,
        threshold,
        workdir,
        false,
        Some(
            "No supported Rust coverage backend is installed. Install `cargo-llvm-cov` or `cargo-tarpaulin`."
                .to_string(),
        ),
    ))
}

struct CommandRun {
    success: bool,
    stdout: String,
    stderr: String,
}

async fn run_with_timeout(
    program: &str,
    args: &[String],
    workdir: &Path,
    timeout_secs: u64,
) -> Result<CommandRun> {
    let mut command = Command::new(program);
    command.args(args);
    command.current_dir(workdir);

    let output = tokio::time::timeout(
        std::time::Duration::from_secs(timeout_secs),
        command.output(),
    )
    .await;

    match output {
        Ok(Ok(out)) => Ok(CommandRun {
            success: out.status.success(),
            stdout: String::from_utf8_lossy(&out.stdout).to_string(),
            stderr: String::from_utf8_lossy(&out.stderr).to_string(),
        }),
        Ok(Err(e)) => Err(anyhow!("test_coverage: failed to spawn process: {e}")),
        Err(_) => Err(anyhow!("test_coverage: timeout after {timeout_secs}s")),
    }
}

fn combined_output(stdout: &str, stderr: &str) -> String {
    match (stdout.trim(), stderr.trim()) {
        ("", "") => String::new(),
        ("", stderr) => stderr.to_string(),
        (stdout, "") => stdout.to_string(),
        (stdout, stderr) => format!("{stdout}\n{stderr}"),
    }
}

fn parse_coverage_pct(output: &str) -> Option<f64> {
    let mut candidate = None;

    for line in output.lines() {
        let lower = line.to_ascii_lowercase();
        if !(lower.contains("total") || lower.contains("coverage")) {
            continue;
        }

        for token in line.split_whitespace() {
            if let Some(raw) = token.strip_suffix('%') {
                let cleaned = raw.trim_matches(|c: char| !c.is_ascii_digit() && c != '.');
                if let Ok(value) = cleaned.parse::<f64>() {
                    candidate = Some(value);
                }
            }
        }
    }

    candidate
}

fn extract_test_summary(output: &str) -> Option<String> {
    output
        .lines()
        .rfind(|line| line.contains("test result:"))
        .map(|line| line.trim().to_string())
}

fn excerpt(output: &str, max_lines: usize) -> String {
    let lines: Vec<&str> = output
        .lines()
        .filter(|line| !line.trim().is_empty())
        .take(max_lines)
        .collect();
    if lines.is_empty() {
        "(no output)".to_string()
    } else {
        lines.join("\n")
    }
}

fn format_report(
    report: CoverageReport,
    threshold: f64,
    workdir: &Path,
    command_success: bool,
    note: Option<String>,
) -> ToolResult {
    let threshold_met = report
        .coverage_pct
        .map(|pct| pct >= threshold)
        .unwrap_or(false);
    let success = note.is_none()
        && command_success
        && report
            .coverage_pct
            .map(|pct| pct >= threshold)
            .unwrap_or(false);

    let mut lines = vec![
        "Project type: Rust".to_string(),
        format!("Coverage backend: {}", report.backend.label()),
        format!("Workdir: {}", workdir.display()),
        format!("Command: {}", report.command),
    ];

    match report.coverage_pct {
        Some(pct) => {
            lines.push(format!("Coverage: {:.2}%", pct));
            lines.push(format!(
                "Threshold: {:.2}% ({})",
                threshold,
                if threshold_met { "met" } else { "failed" }
            ));
        }
        None => {
            lines.push("Coverage: unavailable".to_string());
            lines.push(format!("Threshold: {:.2}% (not evaluated)", threshold));
        }
    }

    lines.push(format!(
        "Command status: {}",
        if command_success { "passed" } else { "failed" }
    ));

    if let Some(summary) = report.test_summary {
        lines.push(format!("Test summary: {summary}"));
    }

    if let Some(note) = note {
        lines.push(format!("Note: {note}"));
    }

    lines.push("Output excerpt:".to_string());
    lines.push(report.raw_excerpt);

    ToolResult {
        output: lines.join("\n"),
        success,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::TempDir;

    #[test]
    fn detects_project_kind() {
        let dir = TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname=\"x\"\nversion=\"0.1.0\"\n",
        )
        .unwrap();
        assert_eq!(detect_project_kind(dir.path()), Some(ProjectKind::Rust));
    }

    #[test]
    fn parses_llvm_cov_total_line() {
        let output = "\
Filename Regions Missed Cover
TOTAL 120 10 91.67%
";
        assert_eq!(parse_coverage_pct(output), Some(91.67));
    }

    #[test]
    fn parses_tarpaulin_coverage_line() {
        let output = "123.45% coverage, 10/10 lines covered";
        assert_eq!(parse_coverage_pct(output), Some(123.45));
    }

    #[test]
    fn extracts_last_test_summary() {
        let output = "\
running 4 tests
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
";
        assert_eq!(
            extract_test_summary(output),
            Some(
                "test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out"
                    .to_string()
            )
        );
    }

    #[test]
    fn format_report_marks_threshold_failure() {
        let report = CoverageReport {
            coverage_pct: Some(72.0),
            test_summary: Some("test result: ok. 10 passed; 0 failed".to_string()),
            raw_excerpt: "coverage output".to_string(),
            command: "cargo tarpaulin --out Stdout".to_string(),
            backend: CoverageBackend::Tarpaulin,
        };
        let formatted = format_report(report, 80.0, Path::new("."), true, None);
        assert!(!formatted.success);
        assert!(formatted.output.contains("Threshold: 80.00% (failed)"));
    }

    #[tokio::test]
    async fn returns_error_for_unknown_project_type() {
        let dir = TempDir::new().unwrap();
        let args = json!({});
        let result = test_coverage(&args, dir.path()).await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("could not detect project type"));
    }

    #[tokio::test]
    async fn node_project_reports_not_implemented_cleanly() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("package.json"), "{\"name\":\"demo\"}").unwrap();
        let args = json!({});
        let result = test_coverage(&args, dir.path()).await.unwrap();
        assert!(!result.success);
        assert!(result
            .output
            .contains("Node coverage is not implemented yet"));
    }
}
