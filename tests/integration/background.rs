use do_it::tools::background::{process_kill, process_list, process_status, run_background};

#[tokio::test]
async fn test_run_background_basic() {
    let args = serde_json::json!({
        "id": "bg-basic",
        "program": "cmd",
        "args": ["/C", "ping", "-n", "6", "127.0.0.1"]
    });
    let root = std::env::current_dir().unwrap();

    let result = run_background(&args, &root).await.unwrap();

    assert!(result.success);
    assert!(result.output.contains("Background process started: PID"));

    let _ = process_kill(&serde_json::json!({"id": "bg-basic"}), &root).await;
}

#[tokio::test]
async fn test_run_background_missing_cmd() {
    let args = serde_json::json!({});
    let root = std::env::current_dir().unwrap();

    let result = run_background(&args, &root).await;

    assert!(result.is_err());
}

#[tokio::test]
async fn test_process_status_with_id() {
    let root = std::env::current_dir().unwrap();
    let start_args = serde_json::json!({
        "id": "bg-status",
        "program": "cmd",
        "args": ["/C", "ping", "-n", "6", "127.0.0.1"]
    });
    run_background(&start_args, &root).await.unwrap();

    let result = process_status(&serde_json::json!({"id": "bg-status"}), &root)
        .await
        .unwrap();

    assert!(result.success);
    assert!(result.output.contains("Command:"));
    assert!(result.output.contains("Id: bg-status"));

    let _ = process_kill(&serde_json::json!({"id": "bg-status"}), &root).await;
}

#[tokio::test]
async fn test_process_status_without_pid() {
    let args = serde_json::json!({});
    let root = std::env::current_dir().unwrap();

    let result = process_status(&args, &root).await.unwrap();

    assert!(result.success);
    assert!(result.output.contains("No matching background process"));
}

#[tokio::test]
async fn test_process_kill_with_id() {
    let root = std::env::current_dir().unwrap();
    let start_args = serde_json::json!({
        "id": "bg-kill",
        "program": "cmd",
        "args": ["/C", "ping", "-n", "6", "127.0.0.1"]
    });
    run_background(&start_args, &root).await.unwrap();

    let result = process_kill(&serde_json::json!({"id": "bg-kill"}), &root)
        .await
        .unwrap();

    assert!(result.success);
    assert!(result.output.contains("Killed process"));
}

#[tokio::test]
async fn test_process_kill_without_pid() {
    let args = serde_json::json!({});
    let root = std::env::current_dir().unwrap();

    let result = process_kill(&args, &root).await.unwrap();

    assert!(!result.success);
    assert!(result.output.contains("not found"));
}

#[tokio::test]
async fn test_process_list() {
    let root = std::env::current_dir().unwrap();
    let start_args = serde_json::json!({
        "id": "bg-list",
        "program": "cmd",
        "args": ["/C", "ping", "-n", "6", "127.0.0.1"]
    });
    run_background(&start_args, &root).await.unwrap();

    let result = process_list(&serde_json::json!({}), &root).await.unwrap();

    assert!(result.success);
    assert!(result.output.contains("Background processes:"));
    assert!(result.output.contains("bg-list"));

    let _ = process_kill(&serde_json::json!({"id": "bg-list"}), &root).await;
}
