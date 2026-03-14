use super::core::{ToolResult, str_arg};
use anyhow::Result;
use serde_json::Value;
use std::path::Path;
use tokio::process::Command;

pub async fn git_status(args: &Value, root: &Path) -> Result<ToolResult> {
    let cwd = if let Some(p) = args.get("cwd").and_then(|v| v.as_str()) {
        super::core::resolve(root, p)?
    } else {
        root.to_path_buf()
    };
    let mut cmd = Command::new("git");
    cmd.arg("status").arg("--porcelain").current_dir(&cwd);
    match cmd.output().await {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let stderr = String::from_utf8_lossy(&out.stderr);
            if out.status.success() {
                if stdout.trim().is_empty() {
                    Ok(ToolResult::ok("Working tree clean"))
                } else {
                    Ok(ToolResult::ok(format!("Git status:\n{}", stdout)))
                }
            } else {
                Ok(ToolResult { output: format!("git status failed: {}", stderr), success: false })
            }
        }
        Err(e) => Ok(ToolResult { output: format!("git status: {}", e), success: false })
    }
}

pub async fn git_commit(args: &Value, root: &Path) -> Result<ToolResult> {
    let message = str_arg(args, "message")?;
    let cwd = if let Some(p) = args.get("cwd").and_then(|v| v.as_str()) {
        super::core::resolve(root, p)?
    } else {
        root.to_path_buf()
    };
    let all = args.get("all").and_then(|v| v.as_bool()).unwrap_or(false);
    let mut cmd = Command::new("git");
    cmd.current_dir(&cwd);
    if all { cmd.arg("add").arg("-A"); }
    cmd.arg("commit").arg("-m").arg(&message);
    match cmd.output().await {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let stderr = String::from_utf8_lossy(&out.stderr);
            if out.status.success() {
                Ok(ToolResult::ok(format!("Committed:\n{}", stdout)))
            } else {
                Ok(ToolResult { output: format!("git commit failed: {}", stderr), success: false })
            }
        }
        Err(e) => Ok(ToolResult { output: format!("git commit: {}", e), success: false })
    }
}

pub async fn git_log(args: &Value, root: &Path) -> Result<ToolResult> {
    let cwd = if let Some(p) = args.get("cwd").and_then(|v| v.as_str()) {
        super::core::resolve(root, p)?
    } else {
        root.to_path_buf()
    };
    let n = args.get("n").and_then(|v| v.as_u64()).unwrap_or(10);
    let mut cmd = Command::new("git");
    cmd.arg("log").arg("--oneline").arg("-n").arg(format!("{}", n)).current_dir(&cwd);
    match cmd.output().await {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            if out.status.success() {
                Ok(ToolResult::ok(format!("Git log (last {}):\n{}", n, stdout)))
            } else {
                let stderr = String::from_utf8_lossy(&out.stderr);
                Ok(ToolResult { output: format!("git log failed: {}", stderr), success: false })
            }
        }
        Err(e) => Ok(ToolResult { output: format!("git log: {}", e), success: false })
    }
}

pub async fn git_stash(args: &Value, root: &Path) -> Result<ToolResult> {
    let action = str_arg(args, "action")?;
    let cwd = if let Some(p) = args.get("cwd").and_then(|v| v.as_str()) {
        super::core::resolve(root, p)?
    } else {
        root.to_path_buf()
    };
    let mut cmd = Command::new("git");
    cmd.current_dir(&cwd);
    match action.as_str() {
        "save" => { cmd.arg("stash").arg("save").arg("-m").arg(args.get("message").and_then(|v| v.as_str()).unwrap_or("work in progress")); }
        "pop" => { cmd.arg("stash").arg("pop"); }
        "list" => { cmd.arg("stash").arg("list"); }
        "drop" => { cmd.arg("stash").arg("drop"); }
        _ => return Ok(ToolResult { output: format!("Unknown stash action: {}", action), success: false })
    }
    match cmd.output().await {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let stderr = String::from_utf8_lossy(&out.stderr);
            if out.status.success() {
                Ok(ToolResult::ok(format!("git stash {}: {}", action, stdout)))
            } else {
                Ok(ToolResult { output: format!("git stash {} failed: {}", action, stderr), success: false })
            }
        }
        Err(e) => Ok(ToolResult { output: format!("git stash: {}", e), success: false })
    }
}

pub async fn git_pull(args: &Value, root: &Path) -> Result<ToolResult> {
    let cwd = if let Some(p) = args.get("cwd").and_then(|v| v.as_str()) {
        super::core::resolve(root, p)?
    } else {
        root.to_path_buf()
    };
    let rebase = args.get("rebase").and_then(|v| v.as_bool()).unwrap_or(false);
    let mut cmd = Command::new("git");
    cmd.current_dir(&cwd).arg("pull");
    if rebase { cmd.arg("--rebase"); }
    match cmd.output().await {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let stderr = String::from_utf8_lossy(&out.stderr);
            if out.status.success() {
                Ok(ToolResult::ok(format!("git pull:\n{}", stdout)))
            } else {
                Ok(ToolResult { output: format!("git pull failed: {}", stderr), success: false })
            }
        }
        Err(e) => Ok(ToolResult { output: format!("git pull: {}", e), success: false })
    }
}

pub async fn git_push(args: &Value, root: &Path) -> Result<ToolResult> {
    let cwd = if let Some(p) = args.get("cwd").and_then(|v| v.as_str()) {
        super::core::resolve(root, p)?
    } else {
        root.to_path_buf()
    };
    let remote = args.get("remote").and_then(|v| v.as_str()).unwrap_or("origin");
    let branch = args.get("branch").and_then(|v| v.as_str());
    let mut cmd = Command::new("git");
    cmd.current_dir(&cwd).arg("push").arg(remote);
    if let Some(b) = branch { cmd.arg(b); }
    match cmd.output().await {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let stderr = String::from_utf8_lossy(&out.stderr);
            if out.status.success() {
                Ok(ToolResult::ok(format!("git push:\n{}", stdout)))
            } else {
                Ok(ToolResult { output: format!("git push failed: {}", stderr), success: false })
            }
        }
        Err(e) => Ok(ToolResult { output: format!("git push: {}", e), success: false })
    }
}
