use super::core::{ToolResult, str_arg};
use super::rate_limit::RateLimiter;
use anyhow::Result;
use serde_json::Value;
use std::io::{self, Write};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

// ─── TUI callback (type-erased to compile in both lib and bin contexts) ───────
//
// We store suspend/resume callbacks rather than a typed TuiEvent sender.
// This avoids referencing do_it::tui::TuiEvent in pub function signatures,
// which would fail in the bin compilation unit where do_it:: is unavailable.

type SuspendFn = Arc<dyn Fn() + Send + Sync>;
type ResumeFn = Arc<dyn Fn() + Send + Sync>;
type StatusFn = Arc<dyn Fn(&str) + Send + Sync>;
type AskFn = Arc<dyn Fn(&str, u64) -> Option<String> + Send + Sync>;

struct TuiCallbacks {
    suspend: SuspendFn,
    resume: ResumeFn,
    status: StatusFn,
    ask: AskFn,
}

fn tui_cbs() -> &'static Mutex<Option<TuiCallbacks>> {
    static TUI_CBS: OnceLock<Mutex<Option<TuiCallbacks>>> = OnceLock::new();
    TUI_CBS.get_or_init(|| Mutex::new(None))
}

fn telegram_cfg() -> &'static Mutex<Option<(String, String)>> {
    static TELEGRAM_CFG: OnceLock<Mutex<Option<(String, String)>>> = OnceLock::new();
    TELEGRAM_CFG.get_or_init(|| Mutex::new(None))
}

/// Set Telegram credentials for this thread. Called by agent loop before dispatch.
pub fn set_telegram_config(token: Option<String>, chat_id: Option<String>) {
    *telegram_cfg()
        .lock()
        .expect("telegram config mutex poisoned") = match (token, chat_id) {
        (Some(t), Some(c)) => Some((t, c)),
        _ => None,
    };
}

fn telegram_credentials() -> Option<(String, String)> {
    // Prefer env vars (explicit override), then thread-local config
    let token = std::env::var("TELEGRAM_BOT_TOKEN").ok();
    let chat_id = std::env::var("TELEGRAM_CHAT_ID").ok();
    if let (Some(t), Some(c)) = (token, chat_id) {
        return Some((t, c));
    }
    telegram_cfg()
        .lock()
        .expect("telegram config mutex poisoned")
        .clone()
}

/// Install TUI callbacks. Called by the agent loop before dispatching a tool.
/// Pass `None` to clear (after dispatch).
pub fn set_tui_callbacks(
    suspend: Option<SuspendFn>,
    resume: Option<ResumeFn>,
    status: Option<StatusFn>,
    ask: Option<AskFn>,
) {
    *tui_cbs().lock().expect("tui callback mutex poisoned") = match (suspend, resume, status, ask) {
        (Some(s), Some(r), Some(st), Some(a)) => Some(TuiCallbacks {
            suspend: s,
            resume: r,
            status: st,
            ask: a,
        }),
        _ => None,
    };
}

fn tui_suspend() {
    if let Some(cbs) = tui_cbs()
        .lock()
        .expect("tui callback mutex poisoned")
        .as_ref()
        .map(|cbs| cbs.suspend.clone())
    {
        cbs();
    }
}

fn tui_resume() {
    if let Some(cbs) = tui_cbs()
        .lock()
        .expect("tui callback mutex poisoned")
        .as_ref()
        .map(|cbs| cbs.resume.clone())
    {
        cbs();
    }
}

fn tui_status(msg: &str) {
    if let Some(cbs) = tui_cbs()
        .lock()
        .expect("tui callback mutex poisoned")
        .as_ref()
        .map(|cbs| cbs.status.clone())
    {
        cbs(msg);
    }
}

fn tui_ask(prompt: &str, timeout_secs: u64) -> Option<String> {
    tui_cbs()
        .lock()
        .expect("tui callback mutex poisoned")
        .as_ref()
        .map(|cbs| cbs.ask.clone())
        .and_then(|cbs| cbs(prompt, timeout_secs))
}

// ─── ask_human ────────────────────────────────────────────────────────────────

/// Pause execution and wait for human input.
/// Args:
///   prompt? / question? — message to display (default: "Waiting for human input...")
///   timeout_secs?       — max seconds to wait (default: 300, 0 = infinite)
pub async fn ask_human(args: &Value) -> Result<ToolResult> {
    let prompt = args
        .get("prompt")
        .or_else(|| args.get("question"))
        .and_then(|v| v.as_str())
        .unwrap_or("Waiting for human input...");
    let timeout_secs = args
        .get("timeout_secs")
        .and_then(|v| v.as_u64())
        .unwrap_or(300);

    {
        let preview: String = prompt.chars().take(60).collect();
        tui_status(&format!("ask_human: {preview}"));
    }

    // When TUI is active: notify Telegram (fire-and-forget) so the user sees
    // the question there too, then collect the answer from the TUI input widget.
    // When TUI is not active: try Telegram interactively first, fall back to console.
    if crate::tui::tui_is_active() {
        // Non-blocking Telegram ping so the user knows input is needed
        if let Some((token, chat_id)) = telegram_credentials() {
            let ping = format!("🤖 ask_human (answer in TUI):\n{prompt}");
            let client = reqwest::Client::builder()
                .timeout(Duration::from_secs(10))
                .build();
            if let Ok(client) = client {
                let url = format!(
                    "{}/bot{}/sendMessage",
                    std::env::var("TELEGRAM_API_BASE_URL")
                        .unwrap_or_else(|_| "https://api.telegram.org".to_string())
                        .trim_end_matches('/'),
                    token
                );
                let payload = serde_json::json!({
                    "chat_id": chat_id,
                    "text": ping,
                    // No parse_mode — plain text avoids Markdown escaping errors
                    // with arbitrary prompt content (code, URLs, special chars).
                });
                // Intentionally fire-and-forget; ignore errors
                let _ = client.post(&url).json(&payload).send().await;
            }
        }
        tui_status("ask_human: waiting for answer in TUI");
        let response = tui_ask(prompt, timeout_secs);
        return match response {
            Some(s) if s.is_empty() => Ok(ToolResult::ok("(no input provided)")),
            Some(s) => Ok(ToolResult::ok(s)),
            None => Ok(ToolResult {
                output: format!("ask_human: timeout after {timeout_secs}s"),
                success: false,
            }),
        };
    }

    // No TUI: try Telegram interactively, then fall back to console
    if let Some((token, chat_id)) = telegram_credentials() {
        match ask_human_via_telegram(prompt, timeout_secs, &token, &chat_id).await {
            Ok(Some(reply)) => return Ok(ToolResult::ok(reply)),
            Ok(None) => {
                tui_status("ask_human: Telegram timeout, falling back to console");
            }
            Err(e) => {
                tracing::warn!("ask_human Telegram failed, falling back to console: {e}");
                tui_status("ask_human: Telegram failed, falling back to console");
            }
        }
    }

    let response = ask_human_via_console(prompt, timeout_secs).await?;

    match response {
        Some(s) if s.is_empty() => Ok(ToolResult::ok("(no input provided)")),
        Some(s) => Ok(ToolResult::ok(s)),
        None => Ok(ToolResult {
            output: format!("ask_human: timeout after {timeout_secs}s"),
            success: false,
        }),
    }
}

async fn ask_human_via_console(prompt: &str, timeout_secs: u64) -> Result<Option<String>> {
    // Suspend TUI so the terminal is usable for stdin
    tui_suspend();

    // Print a clear visual separator so multiple sequential questions
    // don't blur together in the terminal output.
    println!();
    println!("┌─────────────────────────────────────────┐");
    println!("│  ASK HUMAN                              │");
    println!("└─────────────────────────────────────────┘");
    // Print each line of the prompt separately for readability
    for line in prompt.lines() {
        println!("  {line}");
    }
    println!();
    print!("> ");
    io::stdout().flush()?;

    let response = read_line_with_timeout(timeout_secs).await;

    // Resume TUI before returning
    tui_resume();
    Ok(response)
}

async fn read_line_with_timeout(timeout_secs: u64) -> Option<String> {
    if timeout_secs == 0 {
        let mut input = String::new();
        io::stdin().read_line(&mut input).ok()?;
        return Some(input.trim().to_string());
    }

    let result = tokio::time::timeout(
        Duration::from_secs(timeout_secs),
        tokio::task::spawn_blocking(|| {
            let mut input = String::new();
            io::stdin()
                .read_line(&mut input)
                .map(|_| input.trim().to_string())
        }),
    )
    .await;

    match result {
        Ok(Ok(Ok(s))) => Some(s),
        _ => None,
    }
}

async fn ask_human_via_telegram(
    prompt: &str,
    timeout_secs: u64,
    token: &str,
    chat_id: &str,
) -> Result<Option<String>> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(35))
        .build()
        .map_err(|e| anyhow::anyhow!("ask_human: HTTP client: {e}"))?;

    RateLimiter::limit("telegram_api", 1).await;
    let base_url = std::env::var("TELEGRAM_API_BASE_URL")
        .unwrap_or_else(|_| "https://api.telegram.org".to_string());
    let base_url = base_url.trim_end_matches('/');

    let offset = latest_update_id(&client, base_url, token)
        .await?
        .map(|id| id + 1);
    let sent_message_id = send_telegram_question(&client, base_url, token, chat_id, prompt).await?;
    let deadline = if timeout_secs == 0 {
        None
    } else {
        Some(Instant::now() + Duration::from_secs(timeout_secs))
    };

    loop {
        let poll_timeout = deadline
            .map(|end| {
                end.saturating_duration_since(Instant::now())
                    .as_secs()
                    .clamp(1, 30)
            })
            .unwrap_or(30);
        if poll_timeout == 0 {
            return Ok(None);
        }

        let updates = get_telegram_updates(&client, base_url, token, offset, poll_timeout).await?;
        let mut next_offset = offset;

        for update in updates {
            next_offset = Some(update.update_id + 1);
            if let Some(reply) = extract_telegram_reply(&update, chat_id, sent_message_id) {
                return Ok(Some(reply));
            }
        }

        if let Some(next) = next_offset {
            if let Some(deadline) = deadline {
                if Instant::now() >= deadline {
                    return Ok(None);
                }
            }
            return poll_for_reply(
                &client,
                base_url,
                token,
                chat_id,
                sent_message_id,
                next,
                deadline,
            )
            .await;
        }

        if let Some(deadline) = deadline {
            if Instant::now() >= deadline {
                return Ok(None);
            }
        }
    }
}

async fn poll_for_reply(
    client: &reqwest::Client,
    base_url: &str,
    token: &str,
    chat_id: &str,
    sent_message_id: i64,
    mut offset: i64,
    deadline: Option<Instant>,
) -> Result<Option<String>> {
    loop {
        let poll_timeout = deadline
            .map(|end| {
                end.saturating_duration_since(Instant::now())
                    .as_secs()
                    .clamp(1, 30)
            })
            .unwrap_or(30);
        if poll_timeout == 0 {
            return Ok(None);
        }

        let updates =
            get_telegram_updates(client, base_url, token, Some(offset), poll_timeout).await?;
        for update in updates {
            offset = update.update_id + 1;
            if let Some(reply) = extract_telegram_reply(&update, chat_id, sent_message_id) {
                return Ok(Some(reply));
            }
        }

        if let Some(deadline) = deadline {
            if Instant::now() >= deadline {
                return Ok(None);
            }
        }
    }
}

async fn latest_update_id(
    client: &reqwest::Client,
    base_url: &str,
    token: &str,
) -> Result<Option<i64>> {
    let updates = get_telegram_updates(client, base_url, token, None, 1).await?;
    Ok(updates.into_iter().map(|u| u.update_id).max())
}

async fn send_telegram_question(
    client: &reqwest::Client,
    base_url: &str,
    token: &str,
    chat_id: &str,
    prompt: &str,
) -> Result<i64> {
    let url = format!("{base_url}/bot{token}/sendMessage");
    let payload = serde_json::json!({
        "chat_id": chat_id,
        "text": format!("Question from do_it:\n\n{prompt}\n\nReply to this message with your answer."),
        "parse_mode": "Markdown",
        "reply_markup": { "force_reply": true, "selective": true },
    });

    let resp = client
        .post(&url)
        .json(&payload)
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("ask_human: Telegram send failed: {e}"))?;
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        anyhow::bail!("ask_human: Telegram send {status}: {body}");
    }
    let parsed: TelegramApiResponse<TelegramMessage> = serde_json::from_str(&body)
        .map_err(|e| anyhow::anyhow!("ask_human: bad sendMessage response: {e}"))?;
    if !parsed.ok {
        anyhow::bail!("ask_human: Telegram sendMessage returned ok=false");
    }
    parsed.result.map(|msg| msg.message_id).ok_or_else(|| {
        anyhow::anyhow!("ask_human: Telegram sendMessage response missing message_id")
    })
}

async fn get_telegram_updates(
    client: &reqwest::Client,
    base_url: &str,
    token: &str,
    offset: Option<i64>,
    timeout_secs: u64,
) -> Result<Vec<TelegramUpdate>> {
    let url = format!("{base_url}/bot{token}/getUpdates");
    let payload = match offset {
        Some(offset) => serde_json::json!({ "offset": offset, "timeout": timeout_secs }),
        None => serde_json::json!({ "timeout": timeout_secs }),
    };

    let resp = client
        .post(&url)
        .json(&payload)
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("ask_human: Telegram getUpdates failed: {e}"))?;
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        anyhow::bail!("ask_human: Telegram getUpdates {status}: {body}");
    }
    let parsed: TelegramApiResponse<Vec<TelegramUpdate>> = serde_json::from_str(&body)
        .map_err(|e| anyhow::anyhow!("ask_human: bad getUpdates response: {e}"))?;
    if !parsed.ok {
        anyhow::bail!("ask_human: Telegram getUpdates returned ok=false");
    }
    Ok(parsed.result.unwrap_or_default())
}

fn extract_telegram_reply(
    update: &TelegramUpdate,
    chat_id: &str,
    sent_message_id: i64,
) -> Option<String> {
    let message = update.message.as_ref()?;
    if message.chat.id != chat_id {
        return None;
    }
    if message.from.as_ref().is_some_and(|from| from.is_bot) {
        return None;
    }
    let text = message.text.as_deref()?.trim();
    if text.is_empty() {
        return None;
    }

    if message
        .reply_to_message
        .as_ref()
        .is_some_and(|reply| reply.message_id == sent_message_id)
    {
        return Some(text.to_string());
    }

    if message.message_id > sent_message_id {
        return Some(text.to_string());
    }

    None
}

#[derive(Debug, serde::Deserialize)]
struct TelegramApiResponse<T> {
    ok: bool,
    result: Option<T>,
}

#[derive(Debug, serde::Deserialize)]
struct TelegramUpdate {
    update_id: i64,
    message: Option<TelegramMessage>,
}

#[derive(Debug, serde::Deserialize)]
struct TelegramMessage {
    message_id: i64,
    chat: TelegramChat,
    from: Option<TelegramUser>,
    text: Option<String>,
    reply_to_message: Option<Box<TelegramMessage>>,
}

#[derive(Debug, serde::Deserialize)]
struct TelegramChat {
    id: String,
}

#[derive(Debug, serde::Deserialize)]
struct TelegramUser {
    is_bot: bool,
}

// ─── notify ───────────────────────────────────────────────────────────────────

/// Send a notification.
/// Args:
///   message  — notification text
///   silent?  — suppress Telegram notification sound (default: false)
///   channel? — "telegram" (default) | "log"
pub async fn notify(args: &Value) -> Result<ToolResult> {
    let message = str_arg(args, "message")?;
    let channel = args
        .get("channel")
        .and_then(|v| v.as_str())
        .unwrap_or("telegram");
    let silent = args
        .get("silent")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    // Show in TUI status bar (non-blocking, no suspend needed)
    tui_status(&format!("notify: {}", &message[..message.len().min(60)]));

    match channel {
        "telegram" => notify_telegram(&message, silent).await,
        "log" => {
            // TUI status already sent above; plain fallback if no TUI
            if tui_cbs()
                .lock()
                .expect("tui callback mutex poisoned")
                .is_none()
            {
                println!("[notify] {message}");
            }
            Ok(ToolResult::ok(format!("logged: {message}")))
        }
        _ => Ok(ToolResult {
            output: format!("notify: unknown channel '{channel}'. Valid: telegram, log"),
            success: false,
        }),
    }
}

async fn notify_telegram(message: &str, silent: bool) -> Result<ToolResult> {
    let (token, chat_id) = match telegram_credentials() {
        Some(pair) => pair,
        None => {
            if tui_cbs()
                .lock()
                .expect("tui callback mutex poisoned")
                .is_none()
            {
                println!("[notify] {message}");
            }
            return Ok(ToolResult {
                output: "Telegram not configured. Set TELEGRAM_BOT_TOKEN and TELEGRAM_CHAT_ID env vars, or add telegram_token/telegram_chat_id to config.toml".into(),
                success: false,
            });
        }
    };

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| anyhow::anyhow!("notify: HTTP client: {e}"))?;

    RateLimiter::limit("telegram_api", 1).await;

    let base_url = std::env::var("TELEGRAM_API_BASE_URL")
        .unwrap_or_else(|_| "https://api.telegram.org".to_string());
    let url = format!(
        "{}/bot{}/sendMessage",
        base_url.trim_end_matches('/'),
        token
    );
    let payload = serde_json::json!({
        "chat_id": chat_id,
        "text": message,
        "parse_mode": "Markdown",
        "disable_notification": silent,
    });

    match client.post(&url).json(&payload).send().await {
        Ok(resp) if resp.status().is_success() => Ok(ToolResult::ok(format!(
            "Telegram notification sent to chat {chat_id}"
        ))),
        Ok(resp) => {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            Ok(ToolResult {
                output: format!("notify: Telegram {status}: {body}"),
                success: false,
            })
        }
        Err(e) => Ok(ToolResult {
            output: format!("notify: Telegram failed: {e}"),
            success: false,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::extract_telegram_reply;

    #[test]
    fn extract_telegram_reply_accepts_direct_reply() {
        let update: super::TelegramUpdate = serde_json::from_value(serde_json::json!({
            "update_id": 101,
            "message": {
                "message_id": 77,
                "chat": { "id": "12345" },
                "from": { "is_bot": false },
                "text": "yes",
                "reply_to_message": {
                    "message_id": 42,
                    "chat": { "id": "12345" },
                    "from": { "is_bot": true },
                    "text": "Question"
                }
            }
        }))
        .unwrap();

        assert_eq!(
            extract_telegram_reply(&update, "12345", 42).as_deref(),
            Some("yes")
        );
    }

    #[test]
    fn extract_telegram_reply_ignores_other_chats() {
        let update: super::TelegramUpdate = serde_json::from_value(serde_json::json!({
            "update_id": 102,
            "message": {
                "message_id": 77,
                "chat": { "id": "99999" },
                "from": { "is_bot": false },
                "text": "yes"
            }
        }))
        .unwrap();

        assert!(extract_telegram_reply(&update, "12345", 42).is_none());
    }
}
