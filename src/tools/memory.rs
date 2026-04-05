use super::core::{ToolResult, str_arg};
use anyhow::Result;
use crate::redaction;
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

/// Redact sensitive tokens from a tool output message before it is returned
/// to the agent.  Sensitivity annotations such as `[sensitivity: ...]` survive
/// because they do not match any known sensitive-token pattern.
fn redact_output(msg: String) -> String {
    redaction::redact(&msg)
}

// ─── Key validation and path resolution ──────────────────────────────────────

/// Validate a memory key and resolve it to a filesystem path.
///
/// Keys support namespacing with forward slashes, e.g. "knowledge/decisions".
/// Each segment must be alphanumeric, underscores, or hyphens. Traversal
/// attempts ("..") and backslashes are rejected.
fn resolve_memory_path(root: &Path, key: &str) -> Result<PathBuf, ToolResult> {
    if key.is_empty() {
        return Err(ToolResult::failure("Memory key cannot be empty"));
    }
    if key.len() > 200 {
        return Err(ToolResult::failure(
            "Memory key too long (max 200 characters)",
        ));
    }
    if key.contains('\\') || key.contains("..") {
        return Err(ToolResult::failure(
            "Memory key contains invalid characters",
        ));
    }
    for segment in key.split('/') {
        if segment.is_empty() {
            return Err(ToolResult::failure(
                "Memory key must not have leading, trailing, or consecutive slashes",
            ));
        }
        if !segment
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
        {
            return Err(ToolResult::failure(format!(
                "Memory key segment '{segment}' must contain only alphanumeric characters, underscores, and hyphens"
            )));
        }
    }
    Ok(root.join(".ai").join("memory").join(format!("{key}.txt")))
}

// ─── memory_read ─────────────────────────────────────────────────────────────

pub async fn memory_read(args: &Value, root: &Path) -> Result<ToolResult> {
    let key = str_arg(args, "key")?;
    let memory_path = match resolve_memory_path(root, &key) {
        Ok(p) => p,
        Err(t) => return Ok(t),
    };
    if memory_path.exists() {
        match fs::read_to_string(&memory_path) {
            Ok(content) => Ok(ToolResult::ok(content)),
            Err(e) => Ok(ToolResult::failure(format!("Read error: {e}"))),
        }
    } else {
        Ok(ToolResult {
            output: format!("Memory key '{key}' not found"),
            success: false,
        })
    }
}

// ─── memory_write ────────────────────────────────────────────────────────────

pub async fn memory_write(args: &Value, root: &Path) -> Result<ToolResult> {
    let key = str_arg(args, "key")?;
    let content = str_arg(args, "content")?;
    let append = args
        .get("append")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    const MAX_MEMORY_SIZE: usize = 1_048_576; // 1 MB
    if content.len() > MAX_MEMORY_SIZE {
        return Ok(ToolResult::failure(format!(
            "Memory content too large (max {MAX_MEMORY_SIZE} bytes)"
        )));
    }

    let memory_path = match resolve_memory_path(root, &key) {
        Ok(p) => p,
        Err(t) => return Ok(t),
    };
    let sensitivity = crate::path_sensitivity::classify_path_sensitivity(root, &memory_path);
    let sensitivity_tag = sensitivity.outcome_tag();
    tracing::debug!(
        memory_key = %key,
        target_path = %memory_path.display(),
        sensitivity = sensitivity.as_str(),
        "memory_write target classified"
    );

    // Create parent dirs (supports "knowledge/decisions" etc.)
    if let Some(parent) = memory_path.parent() {
        if let Err(e) = fs::create_dir_all(parent) {
            return Ok(ToolResult::failure(format!("memory_write mkdir: {e}")));
        }
    }

    if append {
        let existing = if memory_path.exists() {
            fs::read_to_string(&memory_path).unwrap_or_default()
        } else {
            String::new()
        };
        if existing.len() + content.len() > MAX_MEMORY_SIZE {
            return Ok(ToolResult::failure(format!(
                "Appending would exceed max size ({MAX_MEMORY_SIZE} bytes)"
            )));
        }
        let combined = if existing.is_empty() {
            content.clone()
        } else if existing.ends_with('\n') {
            format!("{existing}{content}")
        } else {
            format!("{existing}\n{content}")
        };
        match fs::write(&memory_path, combined) {
            Ok(_) => Ok(ToolResult::ok(redact_output(format!(
                "Memory '{key}' appended {sensitivity_tag}"
            )))),
            Err(e) => Ok(ToolResult::failure(format!("Write error: {e}"))),
        }
    } else {
        match fs::write(&memory_path, &content) {
            Ok(_) => Ok(ToolResult::ok(redact_output(format!(
                "Memory '{key}' saved {sensitivity_tag}"
            )))),
            Err(e) => Ok(ToolResult::failure(format!("Write error: {e}"))),
        }
    }
}

// ─── memory_delete ───────────────────────────────────────────────────────────

pub async fn memory_delete(args: &Value, root: &Path) -> Result<ToolResult> {
    let key = str_arg(args, "key")?;
    let memory_path = match resolve_memory_path(root, &key) {
        Ok(p) => p,
        Err(t) => return Ok(t),
    };
    let sensitivity = crate::path_sensitivity::classify_path_sensitivity(root, &memory_path);
    let sensitivity_tag = sensitivity.outcome_tag();
    tracing::debug!(
        memory_key = %key,
        target_path = %memory_path.display(),
        sensitivity = sensitivity.as_str(),
        "memory_delete target classified"
    );
    if !memory_path.exists() {
        return Ok(ToolResult::failure(format!("Memory key '{key}' not found")));
    }
    match fs::remove_file(&memory_path) {
        Ok(_) => Ok(ToolResult::ok(redact_output(format!(
            "Memory '{key}' deleted {sensitivity_tag}"
        )))),
        Err(e) => Ok(ToolResult::failure(format!("Delete error: {e}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_memory_write_success() {
        let tmp = TempDir::new().unwrap();
        let result = memory_write(
            &json!({ "key": "test_key", "content": "hello" }),
            tmp.path(),
        )
        .await
        .unwrap();
        assert!(result.success);
        assert!(result.output.contains("test_key"));
        assert!(result.output.contains("[sensitivity: memory]"));
        assert!(tmp.path().join(".ai/memory/test_key.txt").exists());
    }

    #[tokio::test]
    async fn test_memory_write_namespaced_key() {
        let tmp = TempDir::new().unwrap();
        let result = memory_write(
            &json!({ "key": "knowledge/decisions", "content": "use sqlx" }),
            tmp.path(),
        )
        .await
        .unwrap();
        assert!(result.success, "{}", result.output);
        assert!(result.output.contains("[sensitivity: memory]"));
        assert!(
            tmp.path()
                .join(".ai/memory/knowledge/decisions.txt")
                .exists()
        );
    }

    #[tokio::test]
    async fn test_memory_write_append() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        memory_write(&json!({ "key": "log", "content": "line1" }), root)
            .await
            .unwrap();
        let r = memory_write(
            &json!({ "key": "log", "content": "line2", "append": true }),
            root,
        )
        .await
        .unwrap();
        assert!(r.success);
        let text = fs::read_to_string(root.join(".ai/memory/log.txt")).unwrap();
        assert!(text.contains("line1"));
        assert!(text.contains("line2"));
    }

    #[tokio::test]
    async fn test_memory_write_missing_key() {
        let tmp = TempDir::new().unwrap();
        let r = memory_write(&json!({ "content": "x" }), tmp.path()).await;
        assert!(r.is_err());
    }

    #[tokio::test]
    async fn test_memory_write_empty_key() {
        let tmp = TempDir::new().unwrap();
        let r = memory_write(&json!({ "key": "", "content": "x" }), tmp.path())
            .await
            .unwrap();
        assert!(!r.success);
        assert!(r.output.contains("cannot be empty"));
    }

    #[tokio::test]
    async fn test_memory_write_key_too_long() {
        let tmp = TempDir::new().unwrap();
        let r = memory_write(
            &json!({ "key": "a".repeat(201), "content": "x" }),
            tmp.path(),
        )
        .await
        .unwrap();
        assert!(!r.success);
        assert!(r.output.contains("too long"));
    }

    #[tokio::test]
    async fn test_memory_write_key_traversal_rejected() {
        let tmp = TempDir::new().unwrap();
        let r = memory_write(
            &json!({ "key": "../etc/passwd", "content": "x" }),
            tmp.path(),
        )
        .await
        .unwrap();
        assert!(!r.success);
    }

    #[tokio::test]
    async fn test_memory_write_key_invalid_segment() {
        let tmp = TempDir::new().unwrap();
        let r = memory_write(&json!({ "key": "test@key", "content": "x" }), tmp.path())
            .await
            .unwrap();
        assert!(!r.success);
        assert!(r.output.contains("alphanumeric"));
    }

    #[tokio::test]
    async fn test_memory_write_content_too_large() {
        let tmp = TempDir::new().unwrap();
        let r = memory_write(
            &json!({ "key": "big", "content": "a".repeat(1_048_577) }),
            tmp.path(),
        )
        .await
        .unwrap();
        assert!(!r.success);
        assert!(r.output.contains("too large"));
    }

    #[tokio::test]
    async fn test_memory_read_success() {
        let tmp = TempDir::new().unwrap();
        memory_write(&json!({ "key": "r", "content": "hello world" }), tmp.path())
            .await
            .unwrap();
        let r = memory_read(&json!({ "key": "r" }), tmp.path())
            .await
            .unwrap();
        assert!(r.success);
        assert!(r.output.contains("hello world"));
    }

    #[tokio::test]
    async fn test_memory_read_namespaced() {
        let tmp = TempDir::new().unwrap();
        memory_write(
            &json!({ "key": "knowledge/qa_report", "content": "all pass" }),
            tmp.path(),
        )
        .await
        .unwrap();
        let r = memory_read(&json!({ "key": "knowledge/qa_report" }), tmp.path())
            .await
            .unwrap();
        assert!(r.success);
        assert!(r.output.contains("all pass"));
    }

    #[tokio::test]
    async fn test_memory_read_not_found() {
        let tmp = TempDir::new().unwrap();
        let r = memory_read(&json!({ "key": "ghost" }), tmp.path())
            .await
            .unwrap();
        assert!(!r.success);
        assert!(r.output.contains("not found"));
    }

    #[tokio::test]
    async fn test_memory_delete_success() {
        let tmp = TempDir::new().unwrap();
        memory_write(&json!({ "key": "todel", "content": "bye" }), tmp.path())
            .await
            .unwrap();
        let r = memory_delete(&json!({ "key": "todel" }), tmp.path())
            .await
            .unwrap();
        assert!(r.success);
        assert!(r.output.contains("[sensitivity: memory]"));
        assert!(!tmp.path().join(".ai/memory/todel.txt").exists());
    }

    #[tokio::test]
    async fn test_memory_delete_not_found() {
        let tmp = TempDir::new().unwrap();
        let r = memory_delete(&json!({ "key": "ghost" }), tmp.path())
            .await
            .unwrap();
        assert!(!r.success);
        assert!(r.output.contains("not found"));
    }

    #[tokio::test]
    async fn test_memory_delete_namespaced() {
        let tmp = TempDir::new().unwrap();
        memory_write(
            &json!({ "key": "knowledge/old", "content": "stale" }),
            tmp.path(),
        )
        .await
        .unwrap();
        let r = memory_delete(&json!({ "key": "knowledge/old" }), tmp.path())
            .await
            .unwrap();
        assert!(r.success);
        assert!(!tmp.path().join(".ai/memory/knowledge/old.txt").exists());
    }

    #[tokio::test]
    async fn test_memory_write_output_preserves_sensitivity_annotation() {
        // Sensitivity annotation must survive redact_output().
        let tmp = TempDir::new().unwrap();
        let r = memory_write(
            &json!({ "key": "mykey", "content": "hello" }),
            tmp.path(),
        )
        .await
        .unwrap();
        assert!(r.success);
        assert!(
            r.output.contains("[sensitivity: memory]"),
            "sensitivity annotation must be preserved: {}",
            r.output
        );
    }

    #[tokio::test]
    async fn test_memory_write_append_output_preserves_sensitivity_annotation() {
        let tmp = TempDir::new().unwrap();
        memory_write(&json!({ "key": "log", "content": "line1" }), tmp.path())
            .await
            .unwrap();
        let r = memory_write(
            &json!({ "key": "log", "content": "line2", "append": true }),
            tmp.path(),
        )
        .await
        .unwrap();
        assert!(r.success);
        assert!(
            r.output.contains("[sensitivity: memory]"),
            "sensitivity annotation must be preserved on append: {}",
            r.output
        );
    }

    #[tokio::test]
    async fn test_memory_delete_output_preserves_sensitivity_annotation() {
        let tmp = TempDir::new().unwrap();
        memory_write(&json!({ "key": "todel", "content": "bye" }), tmp.path())
            .await
            .unwrap();
        let r = memory_delete(&json!({ "key": "todel" }), tmp.path())
            .await
            .unwrap();
        assert!(r.success);
        assert!(
            r.output.contains("[sensitivity: memory]"),
            "sensitivity annotation must be preserved on delete: {}",
            r.output
        );
    }
}
