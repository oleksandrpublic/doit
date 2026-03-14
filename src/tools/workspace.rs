use super::core::{ToolResult, str_arg};
use anyhow::Result;
use serde_json::Value;
use std::path::Path;
use std::fs;

pub async fn diff_repo(args: &Value, root: &Path) -> Result<ToolResult> {
    let path = args.get("path").and_then(|v| v.as_str()).unwrap_or(".");
    let full_path = root.join(path);
    Ok(ToolResult::ok(format!("Diff: {:?}", full_path)))
}

pub async fn tree(args: &Value, root: &Path) -> Result<ToolResult> {
    let mut output = String::from("Tree:\n");
    if let Ok(entries) = fs::read_dir(root) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if !name.starts_with('.') {
                output.push_str(&format!("  {}\n", name));
            }
        }
    }
    Ok(ToolResult::ok(output))
}
