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

    // Reserved keys are routed to canonical paths outside .ai/memory/ so that
    // the agent prompt, the lifecycle, and `do_it status` all agree on where
    // these files live. Any other key uses .ai/memory/<key>.txt.
    //
    // ┌──────────────────────┬─────────────────────────────────────────────────┐
    // │ key                  │ canonical path                                  │
    // ├──────────────────────┼─────────────────────────────────────────────────┤
    // │ last_session         │ <repo>/.ai/state/last_session.md                │
    // │ plan                 │ <repo>/.ai/state/current_plan.md                │
    // │ external_messages    │ <repo>/.ai/state/external_messages.md           │
    // │ user_profile         │ ~/.do_it/user_profile.md                        │
    // │ boss_notes           │ ~/.do_it/boss_notes.md                          │
    // │ tool_wishlist        │ ~/.do_it/tool_wishlist.md                       │
    // └──────────────────────┴─────────────────────────────────────────────────┘
    match key {
        "last_session" => {
            return Ok(root.join(".ai").join("state").join("last_session.md"));
        }
        "plan" => {
            return Ok(root.join(".ai").join("state").join("current_plan.md"));
        }
        "external_messages" => {
            // Inbox written by the Telegram /inbox poller (human.rs).
            // The agent reads this to receive proactive user messages mid-session.
            return Ok(root.join(".ai").join("state").join("external_messages.md"));
        }
        "user_profile" => {
            let path = crate::config_loader::global_user_profile_path()
                .ok_or_else(|| ToolResult::failure(
                    "Cannot determine home directory for user_profile key",
                ))?;
            return Ok(path);
        }
        "boss_notes" => {
            let path = crate::config_loader::global_boss_notes_path()
                .ok_or_else(|| ToolResult::failure(
                    "Cannot determine home directory for boss_notes key",
                ))?;
            return Ok(path);
        }
        "tool_wishlist" => {
            let path = crate::config_loader::global_tool_wishlist_path()
                .ok_or_else(|| ToolResult::failure(
                    "Cannot determine home directory for tool_wishlist key",
                ))?;
            return Ok(path);
        }
        _ => {}
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

// ─── checkpoint ─────────────────────────────────────────────────────────────

/// Write a progress note to `.ai/state/checkpoints.md`.
///
/// Agents call this mid-task to record "where I am" without finishing.
/// The file is appended; nothing is ever deleted by this function.
/// All writes are best-effort — a disk error does not propagate as a tool
/// failure so that a transient I/O problem never interrupts the agent loop.
pub async fn checkpoint(args: &Value, root: &Path) -> Result<ToolResult> {
    let note = str_arg(args, "note")?;
    let note = note.trim();
    if note.is_empty() {
        return Ok(ToolResult::failure("checkpoint note cannot be empty"));
    }

    let state_dir = root.join(".ai").join("state");
    let _ = fs::create_dir_all(&state_dir);
    let path = state_dir.join("checkpoints.md");

    let now = crate::tools::core::chrono_now();
    let entry = format!("\n## {now}\n{note}\n");

    let existing = fs::read_to_string(&path).unwrap_or_default();
    match fs::write(&path, format!("{existing}{entry}")) {
        Ok(_) => Ok(ToolResult::ok(format!("Checkpoint recorded at {now}"))),
        Err(e) => Ok(ToolResult::failure(format!("checkpoint write error: {e}"))),
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

    #[tokio::test]
    async fn checkpoint_writes_structured_entry() {
        let tmp = TempDir::new().unwrap();
        let r = checkpoint(
            &json!({ "note": "Finished implementing the parser, tests pass" }),
            tmp.path(),
        )
        .await
        .unwrap();

        assert!(r.success, "checkpoint must succeed: {}", r.output);
        assert!(r.output.contains("Checkpoint recorded at"));

        let path = tmp.path().join(".ai").join("state").join("checkpoints.md");
        assert!(path.exists(), "checkpoints.md must be created");
        let content = fs::read_to_string(&path).unwrap();
        assert!(
            content.contains("Finished implementing the parser"),
            "note text must be present: {content}"
        );
    }

    #[tokio::test]
    async fn checkpoint_accumulates_multiple_entries() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();

        checkpoint(&json!({ "note": "Step 1 done" }), root).await.unwrap();
        checkpoint(&json!({ "note": "Step 2 done" }), root).await.unwrap();

        let content = fs::read_to_string(
            root.join(".ai").join("state").join("checkpoints.md")
        ).unwrap();
        assert!(content.contains("Step 1 done"), "first entry must be present");
        assert!(content.contains("Step 2 done"), "second entry must be present");
    }

    #[tokio::test]
    async fn checkpoint_rejects_empty_note() {
        let tmp = TempDir::new().unwrap();
        let r = checkpoint(&json!({ "note": "   " }), tmp.path())
            .await
            .unwrap();
        assert!(!r.success);
        assert!(r.output.contains("cannot be empty"));
    }

    #[tokio::test]
    async fn checkpoint_creates_state_dir_if_missing() {
        let tmp = TempDir::new().unwrap();
        let state_dir = tmp.path().join(".ai").join("state");
        assert!(!state_dir.exists());

        let r = checkpoint(&json!({ "note": "progress note" }), tmp.path())
            .await
            .unwrap();
        assert!(r.success);
        assert!(state_dir.exists(), ".ai/state must be created");
    }

    // ─── Reserved key routing tests ───────────────────────────────────────────

    #[tokio::test]
    async fn reserved_key_last_session_routes_to_state_dir() {
        let tmp = TempDir::new().unwrap();
        let r = memory_write(
            &json!({ "key": "last_session", "content": "done: x\nremaining: y" }),
            tmp.path(),
        )
        .await
        .unwrap();
        assert!(r.success, "{}", r.output);

        let expected = tmp.path().join(".ai").join("state").join("last_session.md");
        let unexpected = tmp.path().join(".ai").join("memory").join("last_session.txt");
        assert!(expected.exists(), "last_session.md must exist in .ai/state/");
        assert!(!unexpected.exists(), "last_session.txt must NOT exist in .ai/memory/");

        let read_r = memory_read(&json!({ "key": "last_session" }), tmp.path())
            .await
            .unwrap();
        assert!(read_r.success);
        assert!(read_r.output.contains("done: x"));
    }

    #[tokio::test]
    async fn reserved_key_plan_routes_to_current_plan_md() {
        let tmp = TempDir::new().unwrap();
        let r = memory_write(
            &json!({ "key": "plan", "content": "## Plan\n1. implement\n2. test" }),
            tmp.path(),
        )
        .await
        .unwrap();
        assert!(r.success, "{}", r.output);

        let expected = tmp.path().join(".ai").join("state").join("current_plan.md");
        let unexpected = tmp.path().join(".ai").join("memory").join("plan.txt");
        assert!(expected.exists(), "current_plan.md must exist in .ai/state/");
        assert!(!unexpected.exists(), "plan.txt must NOT exist in .ai/memory/");

        let read_r = memory_read(&json!({ "key": "plan" }), tmp.path())
            .await
            .unwrap();
        assert!(read_r.success);
        assert!(read_r.output.contains("1. implement"));
    }

    #[tokio::test]
    async fn reserved_key_external_messages_routes_to_state_dir() {
        // Regression: /inbox writes to .ai/state/external_messages.md
        // but memory_read("external_messages") must resolve to the same path.
        let tmp = TempDir::new().unwrap();

        // Simulate what the inbox poller writes directly to disk
        let state_dir = tmp.path().join(".ai").join("state");
        std::fs::create_dir_all(&state_dir).unwrap();
        let inbox_path = state_dir.join("external_messages.md");
        std::fs::write(&inbox_path, "- [2026-04-18] stop current task\n").unwrap();

        // Agent reads via memory_read("external_messages")
        let r = memory_read(&json!({ "key": "external_messages" }), tmp.path())
            .await
            .unwrap();
        assert!(r.success, "external_messages must be readable: {}", r.output);
        assert!(r.output.contains("stop current task"));

        // Memory write must also route to the same path
        let wr = memory_write(
            &json!({ "key": "external_messages", "content": "cleared", "append": false }),
            tmp.path(),
        )
        .await
        .unwrap();
        assert!(wr.success, "{}", wr.output);
        let unexpected = tmp.path().join(".ai").join("memory").join("external_messages.txt");
        assert!(!unexpected.exists(), "must NOT land in .ai/memory/");
    }

    #[tokio::test]
    async fn non_reserved_key_still_uses_memory_dir() {
        let tmp = TempDir::new().unwrap();
        let r = memory_write(
            &json!({ "key": "my_custom_key", "content": "custom content" }),
            tmp.path(),
        )
        .await
        .unwrap();
        assert!(r.success, "{}", r.output);

        let expected = tmp.path().join(".ai").join("memory").join("my_custom_key.txt");
        assert!(expected.exists(), "custom key must land in .ai/memory/");
    }
}
