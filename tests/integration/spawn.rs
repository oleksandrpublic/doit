//! Integration tests for spawn_agent result → ToolResult success mapping.
//!
//! These tests exercise `build_result_text` + `is_sub_agent_stopped` together
//! against a real on-disk memory store, without running a live LLM.
//!
//! The regression they guard:
//!   A sub-agent that writes a stop-marker into its memory_key must cause
//!   `spawn_agent` to return `success=false`, so the boss does not silently
//!   re-delegate the same task.

use do_it::agent::spawn::build_result_text;
use do_it::tools::memory::memory_write;
use tempfile::TempDir;

/// Helper: write a value into the in-repo memory store.
async fn write_memory(root: &std::path::Path, key: &str, value: &str) {
    let args = serde_json::json!({ "key": key, "content": value });
    memory_write(&args, root)
        .await
        .expect("memory_write must not fail in test setup");
}

// ── stop-marker paths ────────────────────────────────────────────────────────

/// When the sub-agent writes a "stopped by user" marker into its memory_key,
/// `build_result_text` must return a string that matches the stop-marker check,
/// which causes `spawn_agent` to return `success=false`.
#[tokio::test]
async fn stopped_by_user_in_memory_produces_failure_text() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    let key = "result/task1";
    write_memory(root, key, "Stopped by user at step 3 — user declined to continue.").await;

    let text = build_result_text(&Some(key.to_string()), root, "developer").await;

    assert!(
        text.to_lowercase().contains("stopped by user"),
        "result text must contain the stop marker: {text}"
    );
    let is_stopped = text.to_lowercase().contains("stopped by user")
        || text.to_lowercase().contains("exited due to repeated errors")
        || text.to_lowercase().contains("stopped due to no progress");
    assert!(is_stopped, "spawn_agent must return success=false for this result: {text}");
}

#[tokio::test]
async fn exited_due_to_repeated_errors_in_memory_produces_failure_text() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    let key = "result/task2";
    write_memory(
        root,
        key,
        "[sub-agent: navigator] exited due to repeated errors: HTTP 500 on every attempt",
    )
    .await;

    let text = build_result_text(&Some(key.to_string()), root, "navigator").await;

    assert!(
        text.to_lowercase().contains("exited due to repeated errors"),
        "result text must contain the stop marker: {text}"
    );
    let is_stopped = text.to_lowercase().contains("stopped by user")
        || text.to_lowercase().contains("exited due to repeated errors")
        || text.to_lowercase().contains("stopped due to no progress");
    assert!(is_stopped, "spawn_agent must return success=false for this result: {text}");
}

// ── normal completion paths ───────────────────────────────────────────────────

/// Normal completion: sub-agent writes a clean result into memory_key.
/// `build_result_text` must return a string with no stop markers.
#[tokio::test]
async fn normal_completion_in_memory_produces_success_text() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    let key = "result/task3";
    write_memory(root, key, "All tests passed. Coverage at 94%.").await;

    let text = build_result_text(&Some(key.to_string()), root, "qa").await;

    assert!(
        text.contains("All tests passed"),
        "result text must contain the sub-agent output: {text}"
    );
    let is_stopped = text.to_lowercase().contains("stopped by user")
        || text.to_lowercase().contains("exited due to repeated errors")
        || text.to_lowercase().contains("stopped due to no progress");
    assert!(!is_stopped, "normal completion must not trigger failure flag: {text}");
}

/// When memory_key is None, `build_result_text` returns the no-key placeholder.
/// This is a normal completion — no stop markers.
#[tokio::test]
async fn no_memory_key_produces_success_text() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();

    let text = build_result_text(&None, root, "developer").await;

    assert!(
        text.contains("no memory_key"),
        "placeholder must mention no memory_key: {text}"
    );
    let is_stopped = text.to_lowercase().contains("stopped by user")
        || text.to_lowercase().contains("exited due to repeated errors")
        || text.to_lowercase().contains("stopped due to no progress");
    assert!(!is_stopped, "no-key placeholder must not trigger failure flag: {text}");
}

/// When memory_key is set but the sub-agent never wrote it, `build_result_text`
/// returns the "empty or unwritten" warning. This is NOT a stop-marker —
/// spawn_agent returns success=true so the boss sees the warning in history.
#[tokio::test]
async fn unwritten_memory_key_produces_warning_not_failure() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    // Deliberately do NOT write the key.

    let text = build_result_text(&Some("result/missing".to_string()), root, "developer").await;

    assert!(
        text.contains("empty or unwritten"),
        "must warn about unwritten key: {text}"
    );
    let is_stopped = text.to_lowercase().contains("stopped by user")
        || text.to_lowercase().contains("exited due to repeated errors")
        || text.to_lowercase().contains("stopped due to no progress");
    assert!(!is_stopped, "unwritten key must not trigger failure flag: {text}");
}
