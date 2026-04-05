use do_it::tools::memory::{memory_read, memory_write};
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
