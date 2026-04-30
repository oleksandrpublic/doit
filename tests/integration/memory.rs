use do_it::tools::memory::{memory_delete, memory_read, memory_write};
use serde_json::json;
use tempfile::TempDir;

#[tokio::test]
async fn test_memory_roundtrip() {
    let temp_dir = TempDir::new().unwrap();
    let write_args = json!({"key": "test", "content": "test content"});
    let write_result = memory_write(&write_args, temp_dir.path()).await.unwrap();
    assert!(write_result.success);
    let read_args = json!({"key": "test"});
    let read_result = memory_read(&read_args, temp_dir.path()).await.unwrap();
    assert!(read_result.success);
    assert!(read_result.output.contains("test content"));
}

/// Reading a key that was never written must fail with a clear message,
/// not panic or return empty success.
#[tokio::test]
async fn read_missing_key_returns_error() {
    let temp_dir = TempDir::new().unwrap();
    let result = memory_read(&json!({"key": "does_not_exist"}), temp_dir.path())
        .await
        .unwrap();
    assert!(
        !result.success,
        "reading a missing key must return success=false, got: {}",
        result.output
    );
    assert!(
        result.output.to_lowercase().contains("not found")
            || result.output.to_lowercase().contains("does not exist")
            || result.output.to_lowercase().contains("no entry"),
        "error message must explain the key is missing: {}",
        result.output
    );
}

/// Path-traversal keys must be rejected before touching the filesystem.
#[tokio::test]
async fn write_traversal_key_is_rejected() {
    let temp_dir = TempDir::new().unwrap();
    let result = memory_write(
        &json!({"key": "../escape", "content": "should not be written"}),
        temp_dir.path(),
    )
    .await
    .unwrap();
    assert!(
        !result.success,
        "traversal key must be rejected, got success with: {}",
        result.output
    );
    // The file must not appear outside the memory directory.
    assert!(
        !temp_dir.path().join("escape").exists(),
        "traversal must not create a file outside the memory directory"
    );
}

/// Namespaced keys (with forward slashes) must round-trip correctly.
#[tokio::test]
async fn namespaced_key_roundtrips() {
    let temp_dir = TempDir::new().unwrap();
    let key = "knowledge/decisions";
    let content = "Use str_replace for targeted edits.";

    let write = memory_write(
        &json!({"key": key, "content": content}),
        temp_dir.path(),
    )
    .await
    .unwrap();
    assert!(write.success, "namespaced write must succeed: {}", write.output);

    let read = memory_read(&json!({"key": key}), temp_dir.path())
        .await
        .unwrap();
    assert!(read.success, "namespaced read must succeed: {}", read.output);
    assert!(
        read.output.contains(content),
        "read output must contain written content: {}",
        read.output
    );
}

/// Deleting a key must make it unreadable.
#[tokio::test]
async fn delete_removes_key() {
    let temp_dir = TempDir::new().unwrap();
    let key = "temp/scratch";

    memory_write(
        &json!({"key": key, "content": "temporary"}),
        temp_dir.path(),
    )
    .await
    .unwrap();

    let del = memory_delete(&json!({"key": key}), temp_dir.path())
        .await
        .unwrap();
    assert!(del.success, "delete must succeed: {}", del.output);

    let read = memory_read(&json!({"key": key}), temp_dir.path())
        .await
        .unwrap();
    assert!(
        !read.success,
        "deleted key must not be readable, got: {}",
        read.output
    );
}
