use super::core::{ToolResult, str_arg};
use anyhow::Result;
use serde_json::Value;
use std::path::Path;
use std::fs;

pub async fn memory_read(args: &Value, root: &Path) -> Result<ToolResult> {
    let key = str_arg(args, "key")?;
    let memory_path = root.join(".ai").join("memory").join(format!("{}.txt", key));
    if memory_path.exists() {
        match fs::read_to_string(&memory_path) {
            Ok(content) => Ok(ToolResult::ok(content)),
            Err(e) => Ok(ToolResult { output: format!("Read error: {}", e), success: false })
        }
    } else {
        Ok(ToolResult { output: format!("Memory key '{}' not found", key), success: false })
    }
}

pub async fn memory_write(args: &Value, root: &Path) -> Result<ToolResult> {
    let key = str_arg(args, "key")?;
    let content = str_arg(args, "content")?;
    let memory_dir = root.join(".ai").join("memory");
    fs::create_dir_all(&memory_dir)?;
    let memory_path = memory_dir.join(format!("{}.txt", key));
    match fs::write(&memory_path, content) {
        Ok(_) => Ok(ToolResult::ok(format!("Memory '{}' saved", key))),
        Err(e) => Ok(ToolResult { output: format!("Write error: {}", e), success: false })
    }
}
