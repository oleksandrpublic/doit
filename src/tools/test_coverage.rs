use super::core::ToolResult;
use anyhow::Result;
use serde_json::Value;
use std::path::Path;
use tokio::process::Command;

pub async fn test_coverage(args: &Value, root: &Path) -> Result<ToolResult> {
    let cwd = if let Some(p) = args.get("cwd").and_then(|v| v.as_str()) {
        super::core::resolve(root, p)?
    } else {
        root.to_path_buf()
    };
    let mut cmd = Command::new("cargo");
    cmd.arg("test").arg("--all").arg("--").arg("--nocapture").current_dir(&cwd);
    match cmd.output().await {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let stderr = String::from_utf8_lossy(&out.stderr);
            if out.status.success() {
                Ok(ToolResult::ok(format!("Tests passed:\n{}", stdout)))
            } else {
                Ok(ToolResult { output: format!("Tests failed:\n{}\n{}", stdout, stderr), success: false })
            }
        }
        Err(e) => Ok(ToolResult { output: format!("cargo test: {}", e), success: false })
    }
}
