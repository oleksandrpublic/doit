use super::core::{ToolResult, str_arg};
use anyhow::Result;
use serde_json::Value;
use std::path::Path;

pub async fn run_background(args: &Value, _root: &Path) -> Result<ToolResult> {
    let cmd = str_arg(args, "cmd")?;
    Ok(ToolResult::ok(format!("Background: {}", cmd)))
}

pub async fn process_status(args: &Value, _root: &Path) -> Result<ToolResult> {
    let pid = args.get("pid").and_then(|v| v.as_u64()).unwrap_or(0);
    Ok(ToolResult::ok(format!("Status: {}", pid)))
}

pub async fn process_kill(args: &Value, _root: &Path) -> Result<ToolResult> {
    let pid = args.get("pid").and_then(|v| v.as_u64()).unwrap_or(0);
    Ok(ToolResult::ok(format!("Killed: {}", pid)))
}

pub async fn process_list(args: &Value, _root: &Path) -> Result<ToolResult> {
    Ok(ToolResult::ok("Process list"))
}
