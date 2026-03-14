use super::core::{ToolResult, str_arg};
use anyhow::Result;
use serde_json::Value;
use std::path::Path;
use tokio::process::Command;

/// Execute an arbitrary command in a subshell.
/// Args:
///   command — shell command string to execute
///   cwd?    — working directory (default: repo root)
///   timeout_secs? — kill after this many seconds (default: 60)
///   env?    — object with environment variables to set
///   shell?  — shell to use (default: platform default: bash/sh/cmd.exe)
///
/// Security: command is passed to a shell. Avoid injection by validating inputs.
pub async fn run_command(args: &Value, root: &Path) -> Result<ToolResult> {
    let cmd = str_arg(args, "command")?;
    
    let cwd = if let Some(p) = args.get("cwd").and_then(|v| v.as_str()) {
        super::core::resolve(root, p)?
    } else {
        root.to_path_buf()
    };
    
    let timeout_secs = args.get("timeout_secs").and_then(|v| v.as_u64()).unwrap_or(60);
    
    // Determine shell based on platform
    let (shell, shell_arg) = if cfg!(windows) {
        ("cmd", "/C")
    } else {
        ("sh", "-c")
    };
    
    let mut command = Command::new(shell);
    command.arg(shell_arg).arg(&cmd).current_dir(&cwd);
    
    // Set environment variables if provided
    if let Some(env_obj) = args.get("env").and_then(|v| v.as_object()) {
        for (key, val) in env_obj {
            if let Some(val_str) = val.as_str() {
                command.env(key, val_str);
            }
        }
    }
    
    // Run with timeout
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(timeout_secs),
        command.output()
    ).await;
    
    match output {
        Ok(Ok(out)) => {
            let stdout = String::from_utf8_lossy(&out.stdout).to_string();
            let stderr = String::from_utf8_lossy(&out.stderr).to_string();
            let success = out.status.success();
            
            let output_text = if success {
                stdout
            } else {
                format!("stdout:\n{}\n\nstderr:\n{}", stdout.trim(), stderr.trim())
            };
            
            Ok(ToolResult {
                output: output_text,
                success,
            })
        }
        Ok(Err(e)) => Ok(ToolResult {
            output: format!("run_command: failed to spawn process: {e}"),
            success: false,
        }),
        Err(_) => Ok(ToolResult {
            output: format!("run_command: timeout after {timeout_secs}s (killed)"),
            success: false,
        }),
    }
}
