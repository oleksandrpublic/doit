use do_it::tools::scripting::run_script;

#[tokio::test]
async fn test_run_script_basic() {
    let dir = tempfile::TempDir::new().unwrap();
    let result = run_script(&serde_json::json!({ "script": "21 * 2" }), dir.path())
        .await
        .unwrap();

    assert!(result.success);
    assert!(result.output.contains("42"));
}

#[tokio::test]
async fn test_run_script_json_and_regex() {
    let dir = tempfile::TempDir::new().unwrap();
    let result = run_script(
        &serde_json::json!({
            "script": r#"
                let obj = parse_json("{\"name\":\"alpha-42\"}");
                regex_match("^alpha-\\d+$", obj["name"])
            "#
        }),
        dir.path(),
    )
    .await
    .unwrap();

    assert!(result.success);
    assert!(result.output.contains("true"));
}
