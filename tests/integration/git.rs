use std::process::Command;
use tempfile::TempDir;

#[test]
fn test_git_operations() {
    let temp_dir = TempDir::new().unwrap();
    let repo_path = temp_dir.path();

    // Initialize git repo
    let output = Command::new("git")
        .args(&["init"])
        .current_dir(repo_path)
        .output()
        .expect("Failed to run git init");

    assert!(output.status.success());

    // Create a test file
    std::fs::write(repo_path.join("test.txt"), "Hello World").unwrap();

    // Add and commit
    Command::new("git")
        .args(&["add", "test.txt"])
        .current_dir(repo_path)
        .output()
        .expect("Failed to run git add");

    Command::new("git")
        .args(&[
            "-c",
            "user.email=test@example.com",
            "-c",
            "user.name=Test User",
            "commit",
            "-m",
            "Initial commit",
        ])
        .current_dir(repo_path)
        .output()
        .expect("Failed to run git commit");

    // Now test our git_log tool (assuming it's implemented)
    // For now, just check that git log works
    let log_output = Command::new("git")
        .args(&["log", "--oneline"])
        .current_dir(repo_path)
        .output()
        .expect("Failed to run git log");

    assert!(log_output.status.success());
    let log = String::from_utf8(log_output.stdout).unwrap();
    assert!(log.contains("Initial commit"));
}
