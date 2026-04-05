use serde_json::json;
use std::path::PathBuf;

mod commands_tool {
    use super::*;
    use do_it::tools::commands::{format_changed_files_only, run_command, run_targeted_test};
    use std::process::Command;
    use tempfile::TempDir;
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
        let root = PathBuf::from(".").canonicalize().unwrap();
        let args = json!({
            "program": "cmd",
            "args": ["/C", "cd"],
            "cwd": "."
        });
        let result = run_command(&args, &root, &[]).await.unwrap();
        assert!(result.success);
    }

    #[tokio::test]
    async fn test_run_command_allowlist_reject() {
        let args = json!({
            "program": "not_allowed",
            "args": ["test"]
        });
        let root = PathBuf::from(".");
        let allowlist = vec!["allowed".to_string()];
        let result = run_command(&args, &root, &allowlist).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_run_command_allowlist_accept() {
        let args = json!({
            "program": "cmd",
            "args": ["/C", "echo", "test"]
        });
        let root = PathBuf::from(".");
        let allowlist = vec!["cmd".to_string()];
        let result = run_command(&args, &root, &allowlist).await.unwrap();
        assert!(result.success);
    }

    #[tokio::test]
    async fn test_run_command_with_env() {
        let args = json!({
            "program": "cmd",
            "args": ["/C", "echo", "%TEST_VAR%"],
            "env": {"TEST_VAR": "test_value"}
        });
        let root = PathBuf::from(".");
        let result = run_command(&args, &root, &[]).await.unwrap();
        assert!(result.success);
        assert!(result.output.contains("test_value"));
    }

    #[tokio::test]
    async fn test_format_changed_files_only_formats_rust_files_in_temp_repo() {
        let temp_dir = TempDir::new().unwrap();
        let repo_path = temp_dir.path();

        std::fs::write(
            repo_path.join("Cargo.toml"),
            "[package]\nname = \"temp_repo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(repo_path.join("src")).unwrap();
        std::fs::write(
            repo_path.join("src").join("lib.rs"),
            "pub fn demo () {println!(\"hi\");}\n",
        )
        .unwrap();

        let init = Command::new("git")
            .args(["init"])
            .current_dir(repo_path)
            .output()
            .unwrap();
        assert!(init.status.success());

        let add = Command::new("git")
            .args(["add", "Cargo.toml", "src/lib.rs"])
            .current_dir(repo_path)
            .output()
            .unwrap();
        assert!(add.status.success());

        let args = json!({
            "timeout_secs": 60
        });
        let root = repo_path.canonicalize().unwrap();
        let result = format_changed_files_only(&args, &root).await.unwrap();
        assert!(result.success, "{}", result.output);
        assert!(
            result.output.contains("Formatted 1 changed Rust file(s)"),
            "{}",
            result.output
        );
        assert!(result.output.contains("src/lib.rs"));

        let formatted = std::fs::read_to_string(repo_path.join("src").join("lib.rs")).unwrap();
        assert!(formatted.contains("pub fn demo()"));
        assert!(formatted.contains("println!(\"hi\");"));
    }

    #[tokio::test]
    #[ignore = "requires manual run; invokes nested cargo test and is slow on Windows"]
    async fn test_run_targeted_test_for_src_file() {
        let root = PathBuf::from(".").canonicalize().unwrap();
        let args = json!({
            "path": "src/tools/commands.rs",
            "test": "test_run_command_echo",
            "timeout_secs": 180
        });
        let result = run_targeted_test(&args, &root).await.unwrap();
        assert!(result.success, "{}", result.output);
        assert!(result
            .output
            .contains("cargo test --lib test_run_command_echo"));
    }
}
