use super::core::{ToolResult, str_arg};
use crate::validation::resolve_safe_path;
use anyhow::Result;
use serde_json::Value;
use std::path::Path;
use tokio::process::Command;

pub async fn git_status(args: &Value, root: &Path) -> Result<ToolResult> {
    let cwd = if let Some(p) = args.get("cwd").and_then(|v| v.as_str()) {
        resolve_safe_path(root, p)?
    } else {
        root.to_path_buf()
    };
    let timeout_secs = args
        .get("timeout_secs")
        .and_then(|v| v.as_u64())
        .unwrap_or(60);
    let mut cmd = Command::new("git");
    cmd.arg("status").arg("--porcelain").current_dir(&cwd);
    let output =
        tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), cmd.output()).await;
    match output {
        Ok(Ok(out)) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let stderr = String::from_utf8_lossy(&out.stderr);
            if out.status.success() {
                if stdout.trim().is_empty() {
                    Ok(ToolResult::ok("Working tree clean"))
                } else {
                    Ok(ToolResult::ok(format!("Git status:\n{}", stdout)))
                }
            } else {
                Ok(ToolResult {
                    output: format!("git status failed: {}", stderr),
                    success: false,
                })
            }
        }
        Ok(Err(e)) => Ok(ToolResult {
            output: format!("git status: {}", e),
            success: false,
        }),
        Err(_) => Ok(ToolResult {
            output: format!("git status: timeout after {timeout_secs}s"),
            success: false,
        }),
    }
}

pub async fn git_commit(args: &Value, root: &Path) -> Result<ToolResult> {
    let message = str_arg(args, "message")?;
    let cwd = if let Some(p) = args.get("cwd").and_then(|v| v.as_str()) {
        resolve_safe_path(root, p)?
    } else {
        root.to_path_buf()
    };
    let timeout_secs = args
        .get("timeout_secs")
        .and_then(|v| v.as_u64())
        .unwrap_or(60);
    let all = args.get("all").and_then(|v| v.as_bool()).unwrap_or(false);
    let mut cmd = Command::new("git");
    cmd.current_dir(&cwd);
    if all {
        cmd.arg("add").arg("-A");
    }
    cmd.arg("commit").arg("-m").arg(&message);
    let output =
        tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), cmd.output()).await;
    match output {
        Ok(Ok(out)) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let stderr = String::from_utf8_lossy(&out.stderr);
            if out.status.success() {
                Ok(ToolResult::ok(format!("Committed:\n{}", stdout)))
            } else {
                Ok(ToolResult {
                    output: format!("git commit failed: {}", stderr),
                    success: false,
                })
            }
        }
        Ok(Err(e)) => Ok(ToolResult {
            output: format!("git commit: {}", e),
            success: false,
        }),
        Err(_) => Ok(ToolResult {
            output: format!("git commit: timeout after {timeout_secs}s"),
            success: false,
        }),
    }
}

pub async fn git_log(args: &Value, root: &Path) -> Result<ToolResult> {
    let cwd = if let Some(p) = args.get("cwd").and_then(|v| v.as_str()) {
        resolve_safe_path(root, p)?
    } else {
        root.to_path_buf()
    };
    let timeout_secs = args
        .get("timeout_secs")
        .and_then(|v| v.as_u64())
        .unwrap_or(60);
    let n = args.get("n").and_then(|v| v.as_u64()).unwrap_or(10);
    let mut cmd = Command::new("git");
    cmd.arg("log")
        .arg("--oneline")
        .arg("-n")
        .arg(format!("{}", n))
        .current_dir(&cwd);
    let output =
        tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), cmd.output()).await;
    match output {
        Ok(Ok(out)) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            if out.status.success() {
                Ok(ToolResult::ok(format!("Git log (last {}):\n{}", n, stdout)))
            } else {
                let stderr = String::from_utf8_lossy(&out.stderr);
                Ok(ToolResult {
                    output: format!("git log failed: {}", stderr),
                    success: false,
                })
            }
        }
        Ok(Err(e)) => Ok(ToolResult {
            output: format!("git log: {}", e),
            success: false,
        }),
        Err(_) => Ok(ToolResult {
            output: format!("git log: timeout after {timeout_secs}s"),
            success: false,
        }),
    }
}

pub async fn git_stash(args: &Value, root: &Path) -> Result<ToolResult> {
    let action = str_arg(args, "action")?;
    let cwd = if let Some(p) = args.get("cwd").and_then(|v| v.as_str()) {
        resolve_safe_path(root, p)?
    } else {
        root.to_path_buf()
    };
    let timeout_secs = args
        .get("timeout_secs")
        .and_then(|v| v.as_u64())
        .unwrap_or(60);
    let mut cmd = Command::new("git");
    cmd.current_dir(&cwd);
    match action.as_str() {
        "save" => {
            cmd.arg("stash").arg("save").arg("-m").arg(
                args.get("message")
                    .and_then(|v| v.as_str())
                    .unwrap_or("work in progress"),
            );
        }
        "pop" => {
            cmd.arg("stash").arg("pop");
        }
        "list" => {
            cmd.arg("stash").arg("list");
        }
        "drop" => {
            cmd.arg("stash").arg("drop");
        }
        _ => {
            return Ok(ToolResult {
                output: format!("Unknown stash action: {}", action),
                success: false,
            });
        }
    }
    let output =
        tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), cmd.output()).await;
    match output {
        Ok(Ok(out)) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let stderr = String::from_utf8_lossy(&out.stderr);
            if out.status.success() {
                Ok(ToolResult::ok(format!("git stash {}: {}", action, stdout)))
            } else {
                Ok(ToolResult {
                    output: format!("git stash {} failed: {}", action, stderr),
                    success: false,
                })
            }
        }
        Ok(Err(e)) => Ok(ToolResult {
            output: format!("git stash: {}", e),
            success: false,
        }),
        Err(_) => Ok(ToolResult {
            output: format!("git stash: timeout after {timeout_secs}s"),
            success: false,
        }),
    }
}

pub async fn git_pull(args: &Value, root: &Path) -> Result<ToolResult> {
    let cwd = if let Some(p) = args.get("cwd").and_then(|v| v.as_str()) {
        resolve_safe_path(root, p)?
    } else {
        root.to_path_buf()
    };
    let timeout_secs = args
        .get("timeout_secs")
        .and_then(|v| v.as_u64())
        .unwrap_or(60);
    let rebase = args
        .get("rebase")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let mut cmd = Command::new("git");
    cmd.current_dir(&cwd).arg("pull");
    if rebase {
        cmd.arg("--rebase");
    }
    let output =
        tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), cmd.output()).await;
    match output {
        Ok(Ok(out)) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let stderr = String::from_utf8_lossy(&out.stderr);
            if out.status.success() {
                Ok(ToolResult::ok(format!("git pull:\n{}", stdout)))
            } else {
                Ok(ToolResult {
                    output: format!("git pull failed: {}", stderr),
                    success: false,
                })
            }
        }
        Ok(Err(e)) => Ok(ToolResult {
            output: format!("git pull: {}", e),
            success: false,
        }),
        Err(_) => Ok(ToolResult {
            output: format!("git pull: timeout after {timeout_secs}s"),
            success: false,
        }),
    }
}

pub async fn git_push(args: &Value, root: &Path) -> Result<ToolResult> {
    let cwd = if let Some(p) = args.get("cwd").and_then(|v| v.as_str()) {
        resolve_safe_path(root, p)?
    } else {
        root.to_path_buf()
    };
    let timeout_secs = args
        .get("timeout_secs")
        .and_then(|v| v.as_u64())
        .unwrap_or(60);
    let remote = args
        .get("remote")
        .and_then(|v| v.as_str())
        .unwrap_or("origin");
    let branch = args.get("branch").and_then(|v| v.as_str());
    let mut cmd = Command::new("git");
    cmd.current_dir(&cwd).arg("push").arg(remote);
    if let Some(b) = branch {
        cmd.arg(b);
    }
    let output =
        tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), cmd.output()).await;
    match output {
        Ok(Ok(out)) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let stderr = String::from_utf8_lossy(&out.stderr);
            if out.status.success() {
                Ok(ToolResult::ok(format!("git push:\n{}", stdout)))
            } else {
                Ok(ToolResult {
                    output: format!("git push failed: {}", stderr),
                    success: false,
                })
            }
        }
        Ok(Err(e)) => Ok(ToolResult {
            output: format!("git push: {}", e),
            success: false,
        }),
        Err(_) => Ok(ToolResult {
            output: format!("git push: timeout after {timeout_secs}s"),
            success: false,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::path::PathBuf;

    #[tokio::test]
    async fn test_git_status_invalid_path() {
        let args = json!({
            "cwd": "../outside_root"
        });
        let root = PathBuf::from(".").canonicalize().unwrap();
        let result = git_status(&args, &root).await;
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Path traversal detected")
        );
    }

    #[tokio::test]
    async fn test_git_commit_missing_message() {
        let args = json!({});
        let root = PathBuf::from(".").canonicalize().unwrap();
        let result = git_commit(&args, &root).await;
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Missing arg: message")
        );
    }

    #[tokio::test]
    async fn test_git_commit_invalid_path() {
        let args = json!({
            "message": "test commit",
            "cwd": "../outside_root"
        });
        let root = PathBuf::from(".").canonicalize().unwrap();
        let result = git_commit(&args, &root).await;
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Path traversal detected")
        );
    }

    #[tokio::test]
    async fn test_git_log_invalid_path() {
        let args = json!({
            "cwd": "../outside_root"
        });
        let root = PathBuf::from(".").canonicalize().unwrap();
        let result = git_log(&args, &root).await;
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Path traversal detected")
        );
    }

    #[tokio::test]
    async fn test_git_stash_unknown_action() {
        let args = json!({
            "action": "unknown_action"
        });
        let root = PathBuf::from(".").canonicalize().unwrap();
        let result = git_stash(&args, &root).await.unwrap();
        assert!(!result.success);
        assert!(result.output.contains("Unknown stash action"));
    }

    #[tokio::test]
    async fn test_git_stash_missing_action() {
        let args = json!({});
        let root = PathBuf::from(".").canonicalize().unwrap();
        let result = git_stash(&args, &root).await;
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Missing arg: action")
        );
    }

    #[tokio::test]
    async fn test_git_pull_invalid_path() {
        let args = json!({
            "cwd": "../outside_root"
        });
        let root = PathBuf::from(".").canonicalize().unwrap();
        let result = git_pull(&args, &root).await;
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Path traversal detected")
        );
    }

    #[tokio::test]
    async fn test_git_push_invalid_path() {
        let args = json!({
            "cwd": "../outside_root"
        });
        let root = PathBuf::from(".").canonicalize().unwrap();
        let result = git_push(&args, &root).await;
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Path traversal detected")
        );
    }
}
