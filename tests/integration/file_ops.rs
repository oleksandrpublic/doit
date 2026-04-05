use do_it::tools::file_ops::apply_patch_preview;
use serde_json::json;
use std::fs;
use tempfile::TempDir;

#[test]
fn test_file_operations() {
    let temp_dir = TempDir::new().unwrap();
    let file_path = temp_dir.path().join("test_file.txt");

    // Write to file
    fs::write(&file_path, "Hello, World!").unwrap();

    // Read from file
    let content = fs::read_to_string(&file_path).unwrap();
    assert_eq!(content, "Hello, World!");

    // Test file exists
    assert!(file_path.exists());

    // Test list dir
    let entries = fs::read_dir(&temp_dir).unwrap();
    let entry_names: Vec<_> = entries
        .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
        .collect();
    assert_eq!(entry_names, vec!["test_file.txt"]);
}

#[test]
fn test_apply_patch_preview_does_not_modify_file() {
    let temp_dir = TempDir::new().unwrap();
    let root = temp_dir.path().to_path_buf();
    let file_path = root.join("preview.txt");
    fs::write(&file_path, "before\nvalue\n").unwrap();

    let args = json!({
        "path": "preview.txt",
        "old_str": "value",
        "new_str": "after"
    });
    let result = apply_patch_preview(&args, &root).unwrap();
    assert!(result.success);
    assert!(result.output.contains("-value"));
    assert!(result.output.contains("+after"));

    let content = fs::read_to_string(&file_path).unwrap();
    assert_eq!(content, "before\nvalue\n");
}
