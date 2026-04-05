use do_it::tools::workspace::{diff_repo, find_entrypoints, project_map, trace_call_path, tree};
use serde_json::json;
use std::process::Command;
use tempfile::TempDir;
use tokio::fs;

#[tokio::test]
async fn test_tree() {
    let temp_dir = TempDir::new().unwrap();
    let args = json!({});
    let result = tree(&args, temp_dir.path()).await.unwrap();
    assert!(result.success);
    assert!(result.output.contains("Tree:"));
}

#[tokio::test]
async fn test_tree_with_files() {
    let temp_dir = TempDir::new().unwrap();
    fs::write(temp_dir.path().join("test.txt"), "content")
        .await
        .unwrap();
    let args = json!({});
    let result = tree(&args, temp_dir.path()).await.unwrap();
    assert!(result.success);
    assert!(result.output.contains("test.txt"));
}

#[tokio::test]
async fn test_diff_repo_default() {
    let temp_dir = TempDir::new().unwrap();
    Command::new("git")
        .arg("init")
        .current_dir(temp_dir.path())
        .output()
        .unwrap();
    Command::new("git")
        .args(["config", "user.email", "test@example.com"])
        .current_dir(temp_dir.path())
        .output()
        .unwrap();
    Command::new("git")
        .args(["config", "user.name", "Test User"])
        .current_dir(temp_dir.path())
        .output()
        .unwrap();
    fs::write(temp_dir.path().join("tracked.txt"), "before\n")
        .await
        .unwrap();
    Command::new("git")
        .args(["add", "."])
        .current_dir(temp_dir.path())
        .output()
        .unwrap();
    Command::new("git")
        .args(["commit", "-m", "init"])
        .current_dir(temp_dir.path())
        .output()
        .unwrap();
    fs::write(temp_dir.path().join("tracked.txt"), "after\n")
        .await
        .unwrap();

    let args = json!({});
    let result = diff_repo(&args, temp_dir.path()).await.unwrap();
    assert!(result.success);
    assert!(result.output.contains("tracked.txt"));
}

#[tokio::test]
async fn test_project_map_summarizes_layout() {
    let temp_dir = TempDir::new().unwrap();
    fs::create_dir(temp_dir.path().join("src")).await.unwrap();
    fs::create_dir(temp_dir.path().join("tests")).await.unwrap();
    fs::write(
        temp_dir.path().join("Cargo.toml"),
        "[package]\nname = \"demo\"\nversion = \"0.1.0\"\n",
    )
    .await
    .unwrap();
    fs::write(
        temp_dir.path().join("src").join("main.rs"),
        "fn main() {}\n",
    )
    .await
    .unwrap();
    fs::write(
        temp_dir.path().join("tests").join("smoke.rs"),
        "#[test]\nfn smoke() {}\n",
    )
    .await
    .unwrap();

    let args = json!({});
    let result = project_map(&args, temp_dir.path()).await.unwrap();
    assert!(result.success);
    assert!(result.output.contains("Project map:"));
    assert!(result.output.contains("Top-level directories: src, tests"));
    assert!(result.output.contains("Key manifests: Cargo.toml"));
    assert!(result
        .output
        .contains("Likely source roots: src (1), tests (1)"));
    assert!(result.output.contains("File types: .rs (2), .toml (1)"));
}

#[tokio::test]
async fn test_find_entrypoints_reports_main_and_tests() {
    let temp_dir = TempDir::new().unwrap();
    fs::create_dir(temp_dir.path().join("src")).await.unwrap();
    fs::create_dir(temp_dir.path().join("tests")).await.unwrap();
    fs::write(
        temp_dir.path().join("src").join("main.rs"),
        "#[tokio::main]\nasync fn main() {\n    let _ = Cli::parse();\n}\n",
    )
    .await
    .unwrap();
    fs::write(
        temp_dir.path().join("tests").join("smoke.rs"),
        "#[test]\nfn smoke() {}\n",
    )
    .await
    .unwrap();

    let args = json!({});
    let result = find_entrypoints(&args, temp_dir.path()).await.unwrap();
    assert!(result.success);
    assert!(result.output.contains("src/main.rs"));
    assert!(result.output.contains("main function"));
    assert!(result.output.contains("tokio main attribute"));
    assert!(result.output.contains("tests/smoke.rs"));
    assert!(result.output.contains("test entrypoint"));
}

#[tokio::test]
async fn test_trace_call_path_reports_call_chain() {
    let temp_dir = TempDir::new().unwrap();
    fs::create_dir(temp_dir.path().join("src")).await.unwrap();
    fs::write(
        temp_dir.path().join("src").join("main.rs"),
        "fn main() {\n    start();\n}\n\nfn start() {\n    process();\n}\n\nfn process() {\n    leaf();\n}\n\nfn leaf() {}\n",
    )
    .await
    .unwrap();

    let args = json!({ "symbol": "leaf" });
    let result = trace_call_path(&args, temp_dir.path()).await.unwrap();
    assert!(result.success);
    assert!(result.output.contains("Call path trace for 'leaf'"));
    assert!(result.output.contains("src/main.rs:13"));
    assert!(result.output.contains("<- src/main.rs:9  process"));
    assert!(result.output.contains("<- src/main.rs:5  start"));
    assert!(result.output.contains("<- src/main.rs:1  main"));
}
