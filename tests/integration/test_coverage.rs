use do_it::tools::test_coverage::test_coverage;

#[tokio::test]
async fn test_test_coverage_node_project_reports_clean_failure() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("package.json"), "{\"name\":\"demo\"}").unwrap();

    let result = test_coverage(&serde_json::json!({}), dir.path())
        .await
        .unwrap();

    assert!(!result.success);
    assert!(result
        .output
        .contains("Node coverage is not implemented yet"));
}

#[tokio::test]
async fn test_test_coverage_unknown_project_returns_error() {
    let dir = tempfile::TempDir::new().unwrap();

    let result = test_coverage(&serde_json::json!({}), dir.path()).await;

    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("could not detect project type"));
}
