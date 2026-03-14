use super::core::{ToolResult, str_arg};
use anyhow::Result;
use serde_json::Value;

/// Pause execution and wait for human input.
/// Args:
///   prompt? — message to display (default: "Waiting for human input...")
///   timeout_secs? — max seconds to wait (default: 300, 0 = infinite)
///
/// Reads from stdin. Returns the human's response.
pub async fn ask_human(args: &Value) -> Result<ToolResult> {
    let prompt = args.get("prompt")
        .and_then(|v| v.as_str())
        .unwrap_or("Waiting for human input...");
    let timeout_secs = args.get("timeout_secs").and_then(|v| v.as_u64()).unwrap_or(300);
    
    println!("\n{prompt}");
    print!("> ");
    use std::io::{self, Write};
    io::stdout().flush()?;
    
    if timeout_secs == 0 {
        // Infinite wait
        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        let response = input.trim().to_string();
        if response.is_empty() {
            return Ok(ToolResult::ok("(no input provided)"));
        }
        return Ok(ToolResult::ok(response));
    }
    
    // Wait with timeout - timeout wraps spawn_blocking, so we get nested Result
    let timeout_result = tokio::time::timeout(
        std::time::Duration::from_secs(timeout_secs),
        tokio::task::spawn_blocking(|| {
            let mut input = String::new();
            io::stdin().read_line(&mut input).map(|_| input.trim().to_string())
        })
    ).await;
    
    // Handle timeout and read result in one match
    let response: String = match timeout_result {
        Ok(Ok(result)) => match result {
            Ok(s) => s,
            Err(e) => return Ok(ToolResult {
                output: format!("ask_human: read error: {e}"),
                success: false,
            }),
        },
        Ok(Err(_)) => return Ok(ToolResult {
            output: "ask_human: task join error".to_string(),
            success: false,
        }),
        Err(_) => return Ok(ToolResult {
            output: format!("ask_human: timeout after {timeout_secs}s"),
            success: false,
        }),
    };
    
    if response.is_empty() {
        Ok(ToolResult::ok("(no input provided)"))
    } else {
        Ok(ToolResult::ok(response))
    }
}

/// Send a notification to a human via configured channel.
/// Args:
///   message — notification text
///   channel? — "telegram", "email", "push" (default: based on config)
///   urgent?  — mark as urgent/high priority (default: false)
///
/// Requires [notify] configuration in config.toml.
pub async fn notify(args: &Value) -> Result<ToolResult> {
    let message = str_arg(args, "message")?;
    let channel = args.get("channel").and_then(|v| v.as_str()).unwrap_or("telegram");
    let urgent = args.get("urgent").and_then(|v| v.as_bool()).unwrap_or(false);
    
    match channel {
        "telegram" => notify_telegram(&message, urgent).await,
        "email" => Ok(ToolResult {
            output: "notify: email channel not yet implemented".into(),
            success: false,
        }),
        "push" => Ok(ToolResult {
            output: "notify: push channel not yet implemented".into(),
            success: false,
        }),
        _ => Ok(ToolResult {
            output: format!("notify: unknown channel '{channel}'. Valid: telegram, email, push"),
            success: false,
        }),
    }
}

async fn notify_telegram(message: &str, urgent: bool) -> Result<ToolResult> {
    // Check for Telegram config
    let token = std::env::var("TELEGRAM_BOT_TOKEN").ok();
    let chat_id = std::env::var("TELEGRAM_CHAT_ID").ok();
    
    let (token, chat_id) = match (token, chat_id) {
        (Some(t), Some(c)) => (t, c),
        _ => {
            return Ok(ToolResult {
                output: "notify: Telegram not configured. Set TELEGRAM_BOT_TOKEN and TELEGRAM_CHAT_ID env vars".into(),
                success: false,
            });
        }
    };
    
    let text = if urgent {
        format!("🚨 URGENT: {message}")
    } else {
        message.to_string()
    };
    
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| anyhow::anyhow!("notify: failed to create HTTP client: {e}"))?;
    
    let url = format!("https://api.telegram.org/bot{token}/sendMessage");
    let payload = serde_json::json!({
        "chat_id": chat_id,
        "text": text,
        "parse_mode": "Markdown"
    });
    
    match client.post(&url).json(&payload).send().await {
        Ok(resp) if resp.status().is_success() => {
            Ok(ToolResult::ok(format!("Telegram notification sent to chat {chat_id}")))
        }
        Ok(resp) => {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            Ok(ToolResult {
                output: format!("notify: Telegram API returned {} {}", status, body),
                success: false,
            })
        }
        Err(e) => Ok(ToolResult {
            output: format!("notify: Telegram request failed: {e}"),
            success: false,
        }),
    }
}
