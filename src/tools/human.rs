use super::core::{ToolResult, str_arg};
use super::rate_limit::RateLimiter;
use anyhow::Result;
use serde_json::Value;
use std::io::{self, Write};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

// ─── TUI callback (type-erased to compile in both lib and bin contexts) ───────
//
// AskSendFn sends TuiEvent::Prompt and returns a oneshot receiver for the
// answer.  It does NOT block — ask_human awaits the receiver itself.
//
// This guarantees that TuiEvent::Prompt is dispatched BEFORE the Telegram
// select! race begins, so the TUI popup always appears regardless of which
// channel answers first.  The old AskFn was blocking (did both send + wait
// inside spawn_blocking), which meant the Prompt event was only sent on the
// first poll of tui_ask_async — after Telegram could already have won the
// select! and dropped the future.

type SuspendFn = Arc<dyn Fn() + Send + Sync>;
type ResumeFn = Arc<dyn Fn() + Send + Sync>;
type StatusFn = Arc<dyn Fn(&str) + Send + Sync>;
type AskSendFn = Arc<
    dyn Fn(&str, u64) -> tokio::sync::oneshot::Receiver<Option<String>> + Send + Sync,
>;
type CancelFn = Arc<dyn Fn() + Send + Sync>;

struct TuiCallbacks {
    suspend: SuspendFn,
    resume: ResumeFn,
    status: StatusFn,
    ask_send: AskSendFn,
    cancel: CancelFn,
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

/// Install TUI callbacks from a raw event sender.
/// Used by spawn_agent to give sub-agents access to the parent TUI
/// without requiring them to own a full TuiHandle.
///
/// Sub-agents have tui=None, so install_tui_callbacks (which requires
/// a TuiHandle) is never called for them. This function allows the
/// spawn_agent dispatcher to forward sub-agent ask_human prompts to
/// the parent TUI using the sender that was installed via set_tui_sender.
pub fn install_tui_callbacks_from_tx(
    tx: tokio::sync::mpsc::UnboundedSender<crate::tui::TuiEvent>,
) {
    use crate::tui::TuiEvent;
    let tx1 = tx.clone();
    let tx2 = tx.clone();
    let tx3 = tx.clone();
    let tx4 = tx.clone();
    let tx5 = tx.clone();
    set_tui_callbacks(
        Some(Arc::new(move || {
            let (ack_tx, ack_rx) = tokio::sync::oneshot::channel::<()>();
            let _ = tx1.send(TuiEvent::Suspend(ack_tx));
            if let Ok(handle) = tokio::runtime::Handle::try_current() {
                let _ = handle.block_on(ack_rx);
            }
        })),
        Some(Arc::new(move || {
            let _ = tx2.send(TuiEvent::Resume);
        })),
        Some(Arc::new(move |msg: &str| {
            let _ = tx3.send(TuiEvent::Status(msg.to_string()));
        })),
        Some(Arc::new(move |prompt: &str, timeout_secs: u64| {
            let (resp_tx, resp_rx) = tokio::sync::oneshot::channel::<Option<String>>();
            let _ = tx4.send(TuiEvent::Prompt {
                prompt: prompt.to_string(),
                timeout_secs,
                response: resp_tx,
            });
            resp_rx
        })),
        Some(Arc::new(move || {
            let _ = tx5.send(TuiEvent::CancelPrompt);
        })),
    );
}

/// Install TUI callbacks. Called by the agent loop before dispatching a tool.
/// Pass `None` to clear (after dispatch).
///
/// `ask_send` sends TuiEvent::Prompt and returns a receiver without blocking.
/// ask_human awaits the receiver directly, so the popup is guaranteed to be
/// shown before the Telegram select! race starts.
pub fn set_tui_callbacks(
    suspend: Option<SuspendFn>,
    resume: Option<ResumeFn>,
    status: Option<StatusFn>,
    ask_send: Option<AskSendFn>,
    cancel: Option<CancelFn>,
) {
    *tui_cbs().lock().expect("tui callback mutex poisoned") =
        match (suspend, resume, status, ask_send, cancel) {
            (Some(s), Some(r), Some(st), Some(a), Some(c)) => Some(TuiCallbacks {
                suspend: s,
                resume: r,
                status: st,
                ask_send: a,
                cancel: c,
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

/// Send TuiEvent::Prompt (non-blocking) and return a receiver for the answer.
/// Returns None if no TUI callbacks are installed.
fn tui_ask_send(
    prompt: &str,
    timeout_secs: u64,
) -> Option<tokio::sync::oneshot::Receiver<Option<String>>> {
    tui_cbs()
        .lock()
        .expect("tui callback mutex poisoned")
        .as_ref()
        .map(|cbs| cbs.ask_send.clone())
        .map(|ask_send| ask_send(prompt, timeout_secs))
}

/// Cancel the active TUI prompt widget (used when Telegram answered first).
fn tui_cancel_prompt() {
    if let Some(cbs) = tui_cbs()
        .lock()
        .expect("tui callback mutex poisoned")
        .as_ref()
        .map(|cbs| cbs.cancel.clone())
    {
        cbs();
    }
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

    // When TUI is active: send Telegram message first (so the user sees it),
    // then race TUI input widget against Telegram poll in parallel.
    // Whichever channel provides an answer first wins; the other is cancelled.
    // When TUI is not active: try Telegram interactively first, then console.
    if crate::tui::tui_is_active() {
        if let Some((token, chat_id)) = telegram_credentials() {
            tui_status("ask_human: sending question to Telegram...");

            let client = reqwest::Client::builder()
                .timeout(Duration::from_secs(35))
                .build()
                .map_err(|e| anyhow::anyhow!("ask_human: HTTP client: {e}"))?;

            RateLimiter::limit("telegram_api", 1).await;
            let base_url = std::env::var("TELEGRAM_API_BASE_URL")
                .unwrap_or_else(|_| "https://api.telegram.org".to_string());
            let base_url_str = base_url.trim_end_matches('/').to_string();

            // Drain all pending updates so poll_for_reply only sees replies
            // that arrive AFTER we send the question. Without this, old
            // messages (e.g. a previous "yes") are delivered as the answer.
            let offset = drain_pending_updates(&client, &base_url_str, &token).await;

            // Send the question to Telegram BEFORE racing with TUI.
            // This guarantees the message is delivered regardless of which
            // channel answers first.
            let sent_message_id = match send_telegram_question(
                &client,
                &base_url_str,
                &token,
                &chat_id,
                prompt,
            )
            .await
            {
                Ok(id) => {
                    tui_status("ask_human: waiting (TUI or Telegram)");
                    Some(id)
                }
                Err(e) => {
                    tracing::warn!("ask_human: Telegram sendMessage failed: {e}");
                    tui_status("ask_human: Telegram send failed, waiting for TUI only");
                    None
                }
            };

            if let Some(msg_id) = sent_message_id {
                let deadline = if timeout_secs == 0 {
                    None
                } else {
                    Some(Instant::now() + Duration::from_secs(timeout_secs))
                };
                let token_clone = token.clone();
                let chat_id_clone = chat_id.clone();
                let base_url_clone = base_url_str.clone();

                // Send the TUI Prompt event BEFORE starting the select! race.
                // This guarantees the popup appears even if Telegram answers
                // before tui_ask_async would have had its first poll.
                let tui_rx = tui_ask_send(prompt, timeout_secs);

                let tg_future = poll_for_reply(
                    &client,
                    &base_url_clone,
                    &token_clone,
                    &chat_id_clone,
                    msg_id,
                    offset,
                    deadline,
                );
                tokio::pin!(tg_future);

                match tui_rx {
                    Some(rx) => {
                        // Race TUI receiver against Telegram poll.
                        // TuiEvent::Prompt was already sent above — popup is shown.
                        let tui_future = async move { rx.await.ok().flatten() };
                        tokio::pin!(tui_future);

                        tokio::select! {
                            tui_result = &mut tui_future => {
                                // TUI answered first — Telegram poll future dropped.
                                return match tui_result {
                                    Some(s) if s.is_empty() => Ok(ToolResult::ok("(no input provided)")),
                                    Some(s) => Ok(ToolResult::ok(s)),
                                    None => Ok(ToolResult {
                                        output: format!("ask_human: timeout after {timeout_secs}s"),
                                        success: false,
                                    }),
                                };
                            }
                            tg_result = &mut tg_future => {
                                match tg_result {
                                    Ok(Some(reply)) => {
                                        // Telegram answered — dismiss TUI prompt and return.
                                        tui_cancel_prompt();
                                        return Ok(ToolResult::ok(reply));
                                    }
                                    Ok(None) => {
                                        // Telegram timed out. TUI still open — wait for it.
                                        tui_status("ask_human: Telegram timed out, waiting for TUI");
                                        let response = tui_future.await;
                                        return match response {
                                            Some(s) if s.is_empty() => Ok(ToolResult::ok("(no input provided)")),
                                            Some(s) => Ok(ToolResult::ok(s)),
                                            None => Ok(ToolResult {
                                                output: format!("ask_human: timeout after {timeout_secs}s"),
                                                success: false,
                                            }),
                                        };
                                    }
                                    Err(e) => {
                                        // Telegram errored. TUI still open — wait for it.
                                        tracing::warn!("ask_human Telegram poll failed: {e}");
                                        tui_status("ask_human: Telegram failed, waiting for TUI");
                                        let response = tui_future.await;
                                        return match response {
                                            Some(s) if s.is_empty() => Ok(ToolResult::ok("(no input provided)")),
                                            Some(s) => Ok(ToolResult::ok(s)),
                                            None => Ok(ToolResult {
                                                output: format!("ask_human: timeout after {timeout_secs}s"),
                                                success: false,
                                            }),
                                        };
                                    }
                                }
                            }
                        }
                    }
                    None => {
                        // No TUI callbacks — wait only for Telegram.
                        match tg_future.await {
                            Ok(Some(reply)) => return Ok(ToolResult::ok(reply)),
                            Ok(None) => {
                                return Ok(ToolResult {
                                    output: format!("ask_human: timeout after {timeout_secs}s"),
                                    success: false,
                                })
                            }
                            Err(e) => {
                                tracing::warn!("ask_human Telegram poll failed: {e}");
                                return Ok(ToolResult {
                                    output: format!("ask_human: Telegram failed: {e}"),
                                    success: false,
                                });
                            }
                        }
                    }
                }
            } else {
                // Telegram send failed — wait for TUI only.
                match tui_ask_send(prompt, timeout_secs) {
                    Some(rx) => {
                        let response = rx.await.ok().flatten();
                        return match response {
                            Some(s) if s.is_empty() => Ok(ToolResult::ok("(no input provided)")),
                            Some(s) => Ok(ToolResult::ok(s)),
                            None => Ok(ToolResult {
                                output: format!("ask_human: timeout after {timeout_secs}s"),
                                success: false,
                            }),
                        };
                    }
                    None => {
                        return Ok(ToolResult {
                            output: "ask_human: Telegram send failed and no TUI available".into(),
                            success: false,
                        });
                    }
                }
            }
        }

        // Telegram not configured — wait for TUI only.
        tui_status("ask_human: waiting for answer in TUI");
        match tui_ask_send(prompt, timeout_secs) {
            Some(rx) => {
                let response = rx.await.ok().flatten();
                return match response {
                    Some(s) if s.is_empty() => Ok(ToolResult::ok("(no input provided)")),
                    Some(s) => Ok(ToolResult::ok(s)),
                    None => Ok(ToolResult {
                        output: format!("ask_human: timeout after {timeout_secs}s"),
                        success: false,
                    }),
                };
            }
            None => {
                return Ok(ToolResult {
                    output: "ask_human: TUI is active but no ask_send callback installed".into(),
                    success: false,
                });
            }
        }
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
    // Suspend TUI so the terminal is usable for stdin.
    // tui_suspend() calls block_on internally, so it must run outside the
    // tokio runtime — use spawn_blocking to avoid "Cannot start a runtime
    // from within a runtime" panic.
    tokio::task::spawn_blocking(tui_suspend).await.ok();

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

    // Drain pending updates first to avoid accepting stale messages.
    let offset = drain_pending_updates(&client, base_url, token).await;
    let sent_message_id = send_telegram_question(&client, base_url, token, chat_id, prompt).await?;
    let deadline = if timeout_secs == 0 {
        None
    } else {
        Some(Instant::now() + Duration::from_secs(timeout_secs))
    };

    poll_for_reply(
        &client,
        base_url,
        token,
        chat_id,
        sent_message_id,
        offset,
        deadline,
    )
    .await
}

/// Drain all pending Telegram updates and return the next offset.
///
/// Calls getUpdates with timeout=0 (non-blocking) in a loop until the server
/// returns an empty result. This advances the offset past any stale messages
/// that arrived before we sent our question.
///
/// Without this, old "yes" replies from previous questions would be accepted
/// as answers to the current question if latest_update_id returned None
/// (e.g. due to a transient network error).
async fn drain_pending_updates(
    client: &reqwest::Client,
    base_url: &str,
    token: &str,
) -> Option<i64> {
    let mut offset: Option<i64> = None;
    // Non-blocking poll: repeat until server returns empty result.
    // Cap iterations to avoid infinite loop on misbehaving server.
    for _ in 0..20 {
        let updates = match get_telegram_updates(client, base_url, token, offset, 0).await {
            Ok(u) => u,
            Err(e) => {
                tracing::debug!("drain_pending_updates: getUpdates error: {e}");
                break;
            }
        };
        if updates.is_empty() {
            break;
        }
        for update in &updates {
            offset = Some(update.update_id + 1);
        }
    }
    offset
}

async fn poll_for_reply(
    client: &reqwest::Client,
    base_url: &str,
    token: &str,
    chat_id: &str,
    sent_message_id: i64,
    mut offset: Option<i64>,
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

        let updates =
            get_telegram_updates(client, base_url, token, offset, poll_timeout).await?;
        for update in updates {
            offset = Some(update.update_id + 1);
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

/// Extract a reply from an update, matching by chat_id and message ordering.
///
/// Acceptance policy (in priority order):
///   1. Direct reply to our question (reply_to_message.message_id == sent_message_id).
///      This is the most reliable match — the user explicitly replied.
///   2. Any later message from the same chat (message_id > sent_message_id).
///      Accepts casual answers that are not formal replies.
///
/// The Telegram Bot API returns `chat.id` as an **integer**, not a string.
/// `ChatId` handles both via `#[serde(untagged)]` and normalises to string
/// for comparison against the configured `chat_id` (which is always a string).
fn extract_telegram_reply(
    update: &TelegramUpdate,
    chat_id: &str,
    sent_message_id: i64,
) -> Option<String> {
    let message = update.message.as_ref()?;
    if message.chat.id.as_str() != chat_id {
        return None;
    }
    if message.from.as_ref().is_some_and(|from| from.is_bot) {
        return None;
    }
    let text = message.text.as_deref()?.trim();
    if text.is_empty() {
        return None;
    }

    // Accept the message if it is a direct reply to our question, OR if it
    // arrived after our question (any message the user sent after we asked).
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

// ─── Telegram API types ───────────────────────────────────────────────────────

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

/// Telegram Bot API returns `chat.id` as a JSON integer (e.g. `397814741`),
/// not a string. Using `#[serde(untagged)]` lets us accept both forms and
/// normalise to string for comparison.
#[derive(Debug, serde::Deserialize)]
#[serde(untagged)]
enum ChatId {
    Int(i64),
    Str(String),
}

impl ChatId {
    /// Normalise to a string for comparison against configured chat_id.
    fn as_str(&self) -> std::borrow::Cow<'_, str> {
        match self {
            ChatId::Int(n) => std::borrow::Cow::Owned(n.to_string()),
            ChatId::Str(s) => std::borrow::Cow::Borrowed(s),
        }
    }
}

#[derive(Debug, serde::Deserialize)]
struct TelegramChat {
    id: ChatId,
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

// ─── Inbox poller ─────────────────────────────────────────────────────────────
//
// Listens for `/inbox <text>` commands from Telegram and appends them to
// `.ai/state/external_messages.md` in the project repo.
//
// This is the async inbox channel: the user can proactively send messages
// to the running agent without waiting for `ask_human` to ask a question.
// The agent reads `external_messages.md` via the `memory_read` tool.
//
// Protocol:
//   - User sends `/inbox Some message here` to the Telegram bot
//   - The poller appends a timestamped line to external_messages.md
//   - The bot replies with "✓ Message recorded." so the user gets confirmation
//   - Any message from the configured chat_id that starts with `/inbox`
//     (case-insensitive) is handled; other messages are ignored
//
// Lifecycle:
//   - Started by `start_inbox_poller()` before the agent loop begins
//   - Runs concurrently with the agent loop using a long-poll interval of 20s
//   - Stops when the shutdown token is cancelled (on agent finish or error)
//   - Uses a separate offset from ask_human; /inbox commands are ignored by
//     ask_human (which looks for plain text replies), so ordering is safe

/// Handle for the inbox poller background task.
/// Call `.stop().await` to cancel the poller and wait for it to finish.
pub struct InboxPollerHandle {
    shutdown: tokio_util::sync::CancellationToken,
    task: tokio::task::JoinHandle<()>,
}

impl InboxPollerHandle {
    /// Signal the poller to stop and wait for it to finish.
    pub async fn stop(self) {
        self.shutdown.cancel();
        let _ = self.task.await;
    }
}

/// Start the Telegram inbox poller in the background.
///
/// Spawns a tokio task that long-polls the Telegram Bot API for `/inbox`
/// commands and appends them to `.ai/state/external_messages.md`.
/// The returned handle must be stopped when the agent session ends.
pub fn start_inbox_poller(
    token: String,
    chat_id: String,
    repo_root: std::path::PathBuf,
) -> InboxPollerHandle {
    let shutdown = tokio_util::sync::CancellationToken::new();
    let shutdown_clone = shutdown.clone();

    let task = tokio::spawn(async move {
        inbox_poller_loop(token, chat_id, repo_root, shutdown_clone).await;
    });

    InboxPollerHandle { shutdown, task }
}

async fn inbox_poller_loop(
    token: String,
    chat_id: String,
    repo_root: std::path::PathBuf,
    shutdown: tokio_util::sync::CancellationToken,
) {
    let base_url = std::env::var("TELEGRAM_API_BASE_URL")
        .unwrap_or_else(|_| "https://api.telegram.org".to_string());
    let base_url = base_url.trim_end_matches('/').to_string();

    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(35))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("inbox_poller: failed to build HTTP client: {e}");
            return;
        }
    };

    // Advance past all existing updates so we don't replay old /inbox messages
    // from before this session started.
    let mut offset: Option<i64> = match drain_pending_updates(&client, &base_url, &token).await {
        offset => offset,
    };

    tracing::info!("inbox_poller: started (chat_id={chat_id})");

    loop {
        if shutdown.is_cancelled() {
            break;
        }

        // Long-poll with 20s timeout. Use select! so shutdown is handled
        // without waiting the full 20s for the HTTP response.
        let poll_fut = get_telegram_updates(&client, &base_url, &token, offset, 20);

        let updates = tokio::select! {
            _ = shutdown.cancelled() => break,
            result = poll_fut => match result {
                Ok(updates) => updates,
                Err(e) => {
                    tracing::debug!("inbox_poller: getUpdates error: {e}");
                    // Back off briefly on errors to avoid hammering the API
                    tokio::select! {
                        _ = shutdown.cancelled() => break,
                        _ = tokio::time::sleep(Duration::from_secs(5)) => {}
                    }
                    continue;
                }
            },
        };

        for update in updates {
            offset = Some(update.update_id + 1);

            let Some(message) = &update.message else {
                continue;
            };

            // Only accept messages from the configured chat
            if message.chat.id.as_str() != chat_id {
                continue;
            }

            // Ignore bots
            if message.from.as_ref().is_some_and(|f| f.is_bot) {
                continue;
            }

            let Some(text) = message.text.as_deref() else {
                continue;
            };

            // Match /inbox command (case-insensitive, with or without @botname suffix)
            let body = parse_inbox_command(text);
            let Some(body) = body else {
                continue;
            };

            if body.is_empty() {
                // /inbox with no body — send a usage hint
                let _ = send_telegram_ack(
                    &client,
                    &base_url,
                    &token,
                    &chat_id,
                    "Usage: /inbox <your message to the agent>",
                )
                .await;
                continue;
            }

            // Append to external_messages.md
            match append_inbox_message(&repo_root, body) {
                Ok(()) => {
                    tracing::info!("inbox_poller: recorded message: {body}");
                    let _ = send_telegram_ack(
                        &client,
                        &base_url,
                        &token,
                        &chat_id,
                        "\u{2713} Message recorded.",
                    )
                    .await;
                }
                Err(e) => {
                    tracing::warn!("inbox_poller: failed to write external_messages.md: {e}");
                    let _ = send_telegram_ack(
                        &client,
                        &base_url,
                        &token,
                        &chat_id,
                        "\u{26a0} Could not record message (write error).",
                    )
                    .await;
                }
            }
        }
    }

    tracing::info!("inbox_poller: stopped");
}

/// Parse `/inbox <body>` or `/inbox@botname <body>` from a Telegram message.
/// Returns `Some(body)` if the message is an /inbox command, `None` otherwise.
/// `body` is trimmed; may be empty if the user sent just `/inbox`.
fn parse_inbox_command(text: &str) -> Option<&str> {
    // Must start with '/'
    let text = text.strip_prefix('/')?;
    // Match "inbox" prefix case-insensitively
    if text.len() < 5 || !text[..5].eq_ignore_ascii_case("inbox") {
        return None;
    }
    let rest = &text[5..];
    // Skip optional @botname suffix (runs until first space or end of string)
    let rest = if rest.starts_with('@') {
        rest.splitn(2, ' ').nth(1).unwrap_or("")
    } else {
        rest
    };
    Some(rest.trim_start_matches(' '))
}

/// Append a timestamped inbox message to `.ai/state/external_messages.md`.
/// Creates the file and directory if they do not exist.
fn append_inbox_message(repo_root: &std::path::Path, body: &str) -> anyhow::Result<()> {
    let state_dir = repo_root.join(".ai").join("state");
    std::fs::create_dir_all(&state_dir)?;
    let path = state_dir.join("external_messages.md");

    let now = crate::tools::chrono_now();
    let line = format!("- [{now}] {body}\n");

    use std::io::Write as _;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?;
    file.write_all(line.as_bytes())?;
    Ok(())
}

/// Send a plain-text acknowledgement back to the Telegram chat (fire-and-forget).
async fn send_telegram_ack(
    client: &reqwest::Client,
    base_url: &str,
    token: &str,
    chat_id: &str,
    text: &str,
) -> anyhow::Result<()> {
    let url = format!("{base_url}/bot{token}/sendMessage");
    let payload = serde_json::json!({
        "chat_id": chat_id,
        "text": text,
    });
    let resp = client.post(&url).json(&payload).send().await?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("inbox_poller: ack sendMessage {status}: {body}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── extract_telegram_reply ─────────────────────────────────────────────

    #[test]
    fn extract_telegram_reply_accepts_direct_reply() {
        let update: TelegramUpdate = serde_json::from_value(serde_json::json!({
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
        let update: TelegramUpdate = serde_json::from_value(serde_json::json!({
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

    #[test]
    fn extract_telegram_reply_accepts_integer_chat_id() {
        let update: TelegramUpdate = serde_json::from_value(serde_json::json!({
            "update_id": 103,
            "message": {
                "message_id": 50,
                "chat": { "id": 397814741_i64 },
                "from": { "is_bot": false },
                "text": "hello from telegram",
                "reply_to_message": {
                    "message_id": 49,
                    "chat": { "id": 397814741_i64 },
                    "from": { "is_bot": true },
                    "text": "Question?"
                }
            }
        }))
        .unwrap();

        assert_eq!(
            extract_telegram_reply(&update, "397814741", 49).as_deref(),
            Some("hello from telegram")
        );
    }

    #[test]
    fn extract_telegram_reply_rejects_integer_chat_id_mismatch() {
        let update: TelegramUpdate = serde_json::from_value(serde_json::json!({
            "update_id": 104,
            "message": {
                "message_id": 55,
                "chat": { "id": 999_i64 },
                "from": { "is_bot": false },
                "text": "sneaky"
            }
        }))
        .unwrap();

        assert!(extract_telegram_reply(&update, "397814741", 50).is_none());
    }

    #[test]
    fn extract_telegram_reply_accepts_later_message_without_reply_to() {
        let update: TelegramUpdate = serde_json::from_value(serde_json::json!({
            "update_id": 105,
            "message": {
                "message_id": 100,
                "chat": { "id": 397814741_i64 },
                "from": { "is_bot": false },
                "text": "ok got it"
            }
        }))
        .unwrap();

        assert_eq!(
            extract_telegram_reply(&update, "397814741", 90).as_deref(),
            Some("ok got it")
        );
    }

    #[test]
    fn extract_telegram_reply_ignores_bot_messages_with_integer_id() {
        let update: TelegramUpdate = serde_json::from_value(serde_json::json!({
            "update_id": 106,
            "message": {
                "message_id": 101,
                "chat": { "id": 397814741_i64 },
                "from": { "is_bot": true },
                "text": "automated response"
            }
        }))
        .unwrap();

        assert!(extract_telegram_reply(&update, "397814741", 90).is_none());
    }

    // ── parse_inbox_command ────────────────────────────────────────────────

    #[test]
    fn inbox_command_plain() {
        assert_eq!(parse_inbox_command("/inbox hello there"), Some("hello there"));
    }

    #[test]
    fn inbox_command_with_bot_suffix() {
        assert_eq!(
            parse_inbox_command("/inbox@mybot stop the current task"),
            Some("stop the current task")
        );
    }

    #[test]
    fn inbox_command_empty_body() {
        assert_eq!(parse_inbox_command("/inbox"), Some(""));
    }

    #[test]
    fn inbox_command_not_inbox() {
        assert_eq!(parse_inbox_command("/start"), None);
        assert_eq!(parse_inbox_command("/help"), None);
        assert_eq!(parse_inbox_command("just a message"), None);
    }

    #[test]
    fn inbox_command_case_insensitive() {
        assert_eq!(parse_inbox_command("/INBOX test"), Some("test"));
        assert_eq!(parse_inbox_command("/Inbox test"), Some("test"));
    }

    #[test]
    fn inbox_command_body_with_leading_spaces() {
        assert_eq!(
            parse_inbox_command("/inbox  leading spaces"),
            Some("leading spaces")
        );
    }
}
