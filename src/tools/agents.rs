use super::core::{ToolResult, str_arg};
use anyhow::Result;
use serde_json::Value;
use std::path::Path;

pub async fn spawn_agent(args: &Value, _root: &Path) -> Result<ToolResult> {
    let role = str_arg(args, "role")?;
    Ok(ToolResult::ok(format!("Spawned agent: {}", role)))
}

pub async fn spawn_agents(args: &Value, _root: &Path) -> Result<ToolResult> {
    let count = args.get("count").and_then(|v| v.as_u64()).unwrap_or(1);
    Ok(ToolResult::ok(format!("Spawned {} agents", count)))
}
