use do_it::tools::core::ToolResult;
use serial_test::serial;
use std::sync::Arc;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

// ─── notify tests ─────────────────────────────────────────────────────────────

#[tokio::test]
#[serial]
async fn test_notify_telegram_success() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/botTEST_TOKEN/sendMessage"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": true,
            "result": { "message_id": 123, "text": "Test message" }
        })))
        .mount(&mock_server)
        .await;

    unsafe {
        std::env::set_var("TELEGRAM_BOT_TOKEN", "TEST_TOKEN");
        std::env::set_var("TELEGRAM_CHAT_ID", "123456");
        std::env::set_var("TELEGRAM_API_BASE_URL", &mock_server.uri());
    }

    let args = serde_json::json!({
        "message": "Test notification",
        "channel": "telegram",
        "silent": false
    });

    let result: ToolResult = do_it::tools::human::notify(&args).await.unwrap();

    assert!(result.success, "output: {}", result.output);
    assert!(result
        .output
        .contains("Telegram notification sent to chat 123456"));

    unsafe {
        std::env::remove_var("TELEGRAM_BOT_TOKEN");
        std::env::remove_var("TELEGRAM_CHAT_ID");
        std::env::remove_var("TELEGRAM_API_BASE_URL");
    }
}

#[tokio::test]
#[serial]
async fn test_notify_telegram_api_error() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/botTEST_TOKEN_ERR/sendMessage"))
        .respond_with(ResponseTemplate::new(400).set_body_string("Bad Request: invalid chat_id"))
        .mount(&mock_server)
        .await;

    unsafe {
        std::env::set_var("TELEGRAM_BOT_TOKEN", "TEST_TOKEN_ERR");
        std::env::set_var("TELEGRAM_CHAT_ID", "invalid_chat");
        std::env::set_var("TELEGRAM_API_BASE_URL", &mock_server.uri());
    }

    let args = serde_json::json!({
        "message": "Test notification",
        "channel": "telegram"
    });

    let result: ToolResult = do_it::tools::human::notify(&args).await.unwrap();

    assert!(!result.success);
    assert!(result.output.contains("400"));

    unsafe {
        std::env::remove_var("TELEGRAM_BOT_TOKEN");
        std::env::remove_var("TELEGRAM_CHAT_ID");
        std::env::remove_var("TELEGRAM_API_BASE_URL");
    }
}

#[tokio::test]
#[serial]
async fn test_notify_telegram_not_configured() {
    unsafe {
        std::env::remove_var("TELEGRAM_BOT_TOKEN");
        std::env::remove_var("TELEGRAM_CHAT_ID");
    }

    let args = serde_json::json!({
        "message": "Test notification",
        "channel": "telegram"
    });

    let result: ToolResult = do_it::tools::human::notify(&args).await.unwrap();

    assert!(!result.success);
    assert!(result.output.contains("Telegram not configured"));
}

#[tokio::test]
#[serial]
async fn test_notify_telegram_silent() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/botTEST_TOKEN_URG/sendMessage"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": true,
            "result": { "message_id": 123, "text": "Silent test" }
        })))
        .mount(&mock_server)
        .await;

    unsafe {
        std::env::set_var("TELEGRAM_BOT_TOKEN", "TEST_TOKEN_URG");
        std::env::set_var("TELEGRAM_CHAT_ID", "123456");
        std::env::set_var("TELEGRAM_API_BASE_URL", &mock_server.uri());
    }

    let args = serde_json::json!({
        "message": "Silent test",
        "channel": "telegram",
        "silent": true
    });

    let result: ToolResult = do_it::tools::human::notify(&args).await.unwrap();

    assert!(result.success, "output: {}", result.output);
    assert!(result.output.contains("Telegram notification sent"));

    unsafe {
        std::env::remove_var("TELEGRAM_BOT_TOKEN");
        std::env::remove_var("TELEGRAM_CHAT_ID");
        std::env::remove_var("TELEGRAM_API_BASE_URL");
    }
}

#[tokio::test]
async fn test_notify_unknown_channel() {
    let args = serde_json::json!({
        "message": "Test notification",
        "channel": "unknown"
    });

    let result: ToolResult = do_it::tools::human::notify(&args).await.unwrap();

    assert!(!result.success);
    assert!(result.output.contains("unknown channel 'unknown'"));
}

#[tokio::test]
async fn test_notify_log_channel() {
    let args = serde_json::json!({
        "message": "Log channel test",
        "channel": "log"
    });

    let result: ToolResult = do_it::tools::human::notify(&args).await.unwrap();

    assert!(result.success);
    assert!(result.output.contains("logged"));
}

#[tokio::test]
async fn test_notify_missing_message() {
    let args = serde_json::json!({
        "channel": "telegram"
    });

    let result = do_it::tools::human::notify(&args).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("message"));
}

// ─── ask_human tests ──────────────────────────────────────────────────────────
// Note: stdin-based tests are skipped in CI (no interactive terminal).
// Timeout behaviour is tested instead.

// ask_human reads stdin which blocks forever in CI (pipe without EOF).
// These tests are marked #[ignore] and must be run manually in an interactive terminal:
//   cargo test --test integration human_test::test_ask_human -- --ignored

#[tokio::test]
#[ignore = "requires interactive terminal — run manually"]
async fn test_ask_human_timeout() {
    let args = serde_json::json!({
        "prompt": "This will time out",
        "timeout_secs": 2
    });
    let result: ToolResult = do_it::tools::human::ask_human(&args).await.unwrap();
    assert!(
        result.output.contains("timeout") || result.output.contains("no input"),
        "unexpected output: {}",
        result.output
    );
}

#[tokio::test]
#[ignore = "requires interactive terminal — run manually"]
async fn test_ask_human_uses_question_alias() {
    let args = serde_json::json!({
        "question": "Test question alias",
        "timeout_secs": 2
    });
    let result: ToolResult = do_it::tools::human::ask_human(&args).await.unwrap();
    assert!(
        result.output.contains("timeout") || result.output.contains("no input"),
        "unexpected output: {}",
        result.output
    );
}

// ─── set_tui_callbacks smoke test ─────────────────────────────────────────────

#[tokio::test]
async fn test_set_tui_callbacks_install_and_clear() {
    // Install no-op callbacks — must not panic.
    //
    // ask_send now returns a oneshot::Receiver instead of Option<String>.
    // A no-op implementation creates a channel and immediately drops the
    // sender — ask_human treats a closed receiver as None (no answer).
    do_it::tools::human::set_tui_callbacks(
        Some(Arc::new(|| {})),
        Some(Arc::new(|| {})),
        Some(Arc::new(|_msg: &str| {})),
        Some(Arc::new(|_prompt: &str, _timeout_secs: u64| {
            let (tx, rx) = tokio::sync::oneshot::channel::<Option<String>>();
            drop(tx); // immediately closed → ask_human receives RecvError → None
            rx
        })),
        Some(Arc::new(|| {})),
    );

    // Verify notify still works with callbacks installed (log channel — no network)
    let args = serde_json::json!({ "message": "hello", "channel": "log" });
    let result: ToolResult = do_it::tools::human::notify(&args).await.unwrap();
    assert!(result.success);

    // Clear callbacks
    do_it::tools::human::set_tui_callbacks(None, None, None, None, None);
}
