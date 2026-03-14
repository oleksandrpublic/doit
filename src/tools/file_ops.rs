use anyhow::{bail, Result};
use serde_json::Value;
use std::path::Path;
use crate::tools::core::{ToolResult, str_arg, resolve};

pub fn read_file(args: &Value, root: &Path) -> Result<ToolResult> {
    let path = resolve(root, &str_arg(args, "path")?)?;
    let content = std::fs::read_to_string(&path)
        .map_err(|e| anyhow::anyhow!("read_file: {e}"))?;
    let start = args.get("start_line").and_then(|v| v.as_u64()).map(|n| n as usize).unwrap_or(1);
    let end = args.get("end_line").and_then(|v| v.as_u64()).map(|n| n as usize);
    let lines: Vec<&str> = content.lines().collect();
    let total = lines.len();
    let s = start.saturating_sub(1).min(total);
    let e = end.map(|n| n.min(total)).unwrap_or((s + 100).min(total));
    let numbered: String = lines[s..e].iter().enumerate()
        .map(|(i, l)| format!("{:>4}  {}", s + i + 1, l)).collect::<Vec<_>>().join("\n");
    Ok(ToolResult::ok(format!("File: {path:?} (lines {}-{} of {})\n{numbered}", s + 1, e, total)))
}

pub fn write_file(args: &Value, root: &Path) -> Result<ToolResult> {
    let path = resolve(root, &str_arg(args, "path")?)?;
    let content = str_arg(args, "content")?;
    let len = content.len();
    if let Some(parent) = path.parent() { std::fs::create_dir_all(parent)?; }
    std::fs::write(&path, content)?;
    Ok(ToolResult::ok(format!("Written {len} bytes to {path:?}")))
}

pub fn str_replace(args: &Value, root: &Path) -> Result<ToolResult> {
    let path = resolve(root, &str_arg(args, "path")?)?;
    let old_str = str_arg(args, "old_str")?;
    let new_str = str_arg(args, "new_str")?;
    let content = std::fs::read_to_string(&path)
        .map_err(|e| anyhow::anyhow!("str_replace: {e}"))?;
    let count = content.matches(&old_str).count();
    if count == 0 { bail!("old_str not found"); }
    if count > 1 { bail!("old_str found {count} times - must be unique"); }
    std::fs::write(&path, content.replacen(&old_str, &new_str, 1))?;
    Ok(ToolResult::ok(format!("Replaced in {path:?}")))
}

pub fn list_dir(args: &Value, root: &Path) -> Result<ToolResult> {
    let dir = if let Some(p) = args.get("path").and_then(|v| v.as_str()) {
        resolve(root, p)?
    } else { root.to_path_buf() };
    let mut entries: Vec<String> = std::fs::read_dir(&dir)
        .map_err(|e| anyhow::anyhow!("list_dir: {e}"))?
        .filter_map(|e| e.ok()).map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            let ft = e.file_type().ok();
            if ft.map(|t| t.is_dir()).unwrap_or(false) { format!("{name}/") } else { name }
        }).collect();
    entries.sort();
    Ok(ToolResult::ok(format!("{}:\n{}", dir.display(), entries.join("\n"))))
}

pub fn find_files(args: &Value, root: &Path) -> Result<ToolResult> {
    let pattern = str_arg(args, "pattern")?;
    let dir = if let Some(p) = args.get("dir").and_then(|v| v.as_str()) {
        resolve(root, p)?
    } else { root.to_path_buf() };
    let pattern_lower = pattern.to_lowercase();
    let (prefix, suffix) = if let Some(s) = pattern.strip_prefix('*') { ("", s)
    } else if let Some(p) = pattern.strip_suffix('*') { (p, "")
    } else { ("", pattern.as_str()) };
    let mut results = Vec::new();
    for entry in walkdir::WalkDir::new(&dir).follow_links(false).into_iter().filter_map(|e| e.ok()) {
        if entry.file_type().is_file() {
            let name = entry.file_name().to_string_lossy().to_lowercase();
            let matches = if prefix.is_empty() && suffix.is_empty() { name.contains(&pattern_lower)
            } else if prefix.is_empty() { name.ends_with(suffix)
            } else { name.starts_with(prefix) };
            if matches {
                let rel = entry.path().strip_prefix(&dir).unwrap_or(entry.path());
                results.push(rel.to_string_lossy().to_string());
            }
        }
    }
    results.sort();
    Ok(ToolResult::ok(results.join("\n")))
}

pub fn search_in_files(args: &Value, root: &Path) -> Result<ToolResult> {
    let pattern = str_arg(args, "pattern")?;
    let dir = if let Some(p) = args.get("dir").and_then(|v| v.as_str()) {
        resolve(root, p)?
    } else { root.to_path_buf() };
    let ext = args.get("ext").and_then(|v| v.as_str());
    let mut results = Vec::new();
    for entry in walkdir::WalkDir::new(&dir).follow_links(false).into_iter().filter_map(|e| e.ok()) {
        if entry.file_type().is_file() {
            let path = entry.path();
            if let Some(e) = ext {
                let ext_matches = path.extension()
                    .map(|s| s.to_string_lossy().to_lowercase())
                    .map(|s| s.as_str() == e)
                    .unwrap_or(false);
                if !ext_matches { continue; }
            }
            if let Ok(content) = std::fs::read_to_string(path) {
                for (i, line) in content.lines().enumerate() {
                    if line.contains(&pattern) {
                        let rel = path.strip_prefix(&dir).unwrap_or(path);
                        results.push(format!("{}:{}: {}", rel.display(), i + 1, line.trim()));
                    }
                }
            }
        }
    }
    Ok(ToolResult::ok(results.join("\n")))
}
