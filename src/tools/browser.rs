//! AWP browser client.
//!
//! WebSocket client for the AWP v0.1 protocol.
//!
//! AWP wire format (WebSocket text frames, UTF-8 JSON):
//!
//! Request  (client -> server):
//!   { "id": "<str>", "type": "request", "method": "<method>", "params": { ... } }
//!
//! Response (server -> client):
//!   { "id": "<str>", "type": "response", "result": { ... } }
//!   { "id": "<str>", "type": "response", "error": { "code": "<str>", "message": "<str>" } }
//!
//! Session lifecycle per connection:
//!   1. connect WebSocket to ws://host:port/
//!   2. awp.hello  — handshake (MUST be first message)
//!   3. session.create  — get session_id
//!   4. page.navigate / page.observe / page.act / page.extract
//!   5. session.close — MUST send before closing WebSocket
//!   6. Close WebSocket with proper Close frame
//!
//! # Configuration
//! Set `[browser] awp_url` in config.toml, e.g.:
//!   [browser]
//!   awp_url = "ws://127.0.0.1:9222"
//!
//! http:// and https:// prefixes are also accepted and silently converted.
//!
//! # Diagnostics
//! Enable detailed tracing with:
//!   RUST_LOG=do_it::tools::browser=debug

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result, bail};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio_tungstenite::{connect_async, tungstenite::Message};

use crate::config_struct::BrowserConfig;
use super::core::{ToolResult, resolve, str_arg};

// ─── Request ID counter ───────────────────────────────────────────────────────

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

fn next_id() -> String {
    NEXT_ID.fetch_add(1, Ordering::Relaxed).to_string()
}

// ─── AWP message envelope ─────────────────────────────────────────────────────

/// Outgoing AWP request frame.
#[derive(Serialize, Debug)]
struct AwpRequest<'a> {
    id: String,
    #[serde(rename = "type")]
    msg_type: &'a str,
    method: &'a str,
    params: Value,
}

/// Incoming AWP response frame.
#[derive(Deserialize, Debug)]
struct AwpResponse {
    id: Option<String>,
    result: Option<Value>,
    error: Option<AwpErrorBody>,
}

#[derive(Deserialize, Debug)]
struct AwpErrorBody {
    code: String,
    message: String,
}

// ─── wait_ms helper ──────────────────────────────────────────────────────────

/// Default wait after navigation/action — gives JS time to settle.
const DEFAULT_WAIT_MS: u64 = 500;
/// Maximum allowed wait — prevents accidental multi-minute pauses.
const MAX_WAIT_MS: u64 = 10_000;

/// Read wait_ms from args, clamp to [0, MAX_WAIT_MS], default to `default`.
fn read_wait_ms(args: &Value, default: u64) -> u64 {
    args.get("wait_ms")
        .and_then(|v| v.as_u64())
        .unwrap_or(default)
        .min(MAX_WAIT_MS)
}

// ─── Client ───────────────────────────────────────────────────────────────────

/// Stateless AWP client. Opens a fresh WebSocket connection per tool call,
/// performs the AWP handshake and session lifecycle, then closes.
/// v0.1 is single-session-per-connection so this is the correct pattern.
pub struct AwpClient {
    ws_url: String,
}

impl AwpClient {
    pub fn new(url: &str) -> Self {
        // Normalise scheme: accept http(s):// and ws(s):// interchangeably.
        let ws_url = if url.starts_with("http://") {
            url.replacen("http://", "ws://", 1)
        } else if url.starts_with("https://") {
            url.replacen("https://", "wss://", 1)
        } else {
            url.to_string()
        };
        // AWP spec: connect to ws://host:port/ (trailing slash required)
        let ws_url = format!("{}/", ws_url.trim_end_matches('/'));
        Self { ws_url }
    }

    /// Try a lightweight connection to check if AWP is reachable.
    /// Returns true when the server accepts a WebSocket connection and handshake.
    pub async fn is_reachable(&self) -> bool {
        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            connect_async(&self.ws_url),
        )
        .await
        .map(|r| r.is_ok())
        .unwrap_or(false)
    }

    /// Open a WebSocket connection, perform AWP handshake, create a session,
    /// run `f`, send session.close, then send a proper WebSocket Close frame.
    ///
    /// `f` receives `&mut AwpSession` so the session is still owned here after
    /// `f` returns. This guarantees `session.close()` is always called, which
    /// prevents "Connection reset without closing handshake" errors on the
    /// AWP server side.
    pub async fn with_session<F, T>(&self, f: F) -> Result<T>
    where
        F: for<'s> FnOnce(&'s mut AwpSession) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<T>> + Send + 's>>,
    {
        tracing::debug!(url = %self.ws_url, "AWP: connecting");
        let (ws_stream, _) = connect_async(&self.ws_url)
            .await
            .with_context(|| format!("AWP: cannot connect to {}", self.ws_url))?;
        tracing::debug!(url = %self.ws_url, "AWP: connected");

        let (sink, stream) = ws_stream.split();
        let mut session = awp_open_session(sink, stream).await?;
        tracing::debug!(session_id = %session.session_id, "AWP: session ready");

        // Run caller logic. Session is borrowed mutably — caller can use all
        // AwpSession methods. After f() returns we still own the session.
        let result = f(&mut session).await;

        if let Err(ref e) = result {
            tracing::debug!(session_id = %session.session_id, error = %e, "AWP: tool call failed, closing session");
        }

        // Always close properly — sends session.close + WebSocket Close frame
        // and drains the server's Close frame. This is the fix for the root
        // cause: before this change with_session never called session.close().
        session.close().await;

        result
    }
}

// ─── Session ──────────────────────────────────────────────────────────────────

type WsSink = futures_util::stream::SplitSink<
    tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    Message,
>;
type WsStream = futures_util::stream::SplitStream<
    tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
>;

/// Live AWP session. Automatically injects `session_id` into every call.
/// On drop, sends session.close followed by a WebSocket Close frame so the
/// server does not log "Connection reset without closing handshake".
pub struct AwpSession {
    session_id: String,
    sink: WsSink,
    stream: WsStream,
}

impl AwpSession {
    async fn call(&mut self, method: &str, mut params: Value) -> Result<Value> {
        if let Some(obj) = params.as_object_mut() {
            obj.insert(
                "session_id".to_string(),
                Value::String(self.session_id.clone()),
            );
        }
        let id = next_id();
        let req = AwpRequest {
            id: id.clone(),
            msg_type: "request",
            method,
            params,
        };
        tracing::debug!(session_id = %self.session_id, method, req_id = %id, "AWP: sending request");
        let frame = serde_json::to_string(&req).expect("serialize awp request");
        self.sink
            .send(Message::Text(frame.into()))
            .await
            .with_context(|| format!("AWP: send {method} failed"))?;

        loop {
            match self.stream.next().await {
                None => bail!("AWP: connection closed waiting for {method} response"),
                Some(Err(e)) => bail!("AWP: WebSocket error during {method}: {e}"),
                Some(Ok(Message::Text(raw))) => {
                    let resp: AwpResponse = serde_json::from_str(&raw)
                        .with_context(|| {
                            format!("AWP: non-JSON response to {method}: {raw}")
                        })?;
                    if resp.id.as_deref() != Some(&id) {
                        tracing::debug!(
                            session_id = %self.session_id,
                            got_id = ?resp.id,
                            want_id = %id,
                            "AWP: skipping unmatched response"
                        );
                        continue;
                    }
                    if let Some(ref err) = resp.error {
                        tracing::debug!(
                            session_id = %self.session_id,
                            method,
                            code = %err.code,
                            msg = %err.message,
                            "AWP: server returned error"
                        );
                        bail!("AWP [{method}] error [{}]: {}", err.code, err.message);
                    }
                    tracing::debug!(session_id = %self.session_id, method, "AWP: response OK");
                    return Ok(resp.result.unwrap_or(Value::Null));
                }
                Some(Ok(Message::Close(_))) => {
                    tracing::debug!(
                        session_id = %self.session_id,
                        method,
                        "AWP: server sent Close frame while waiting for response"
                    );
                    bail!("AWP: server closed the connection while waiting for {method} response");
                }
                Some(Ok(_)) => {}
            }
        }
    }

    /// Send session.close then WebSocket Close frame and wait for the server's
    /// Close frame. This prevents "Connection reset without closing handshake"
    /// errors on the server side.
    ///
    /// Must be called before the session is dropped. If not called explicitly,
    /// the server will see an abrupt TCP disconnect.
    pub async fn close(mut self) {
        tracing::debug!(session_id = %self.session_id, "AWP: closing session");

        // 1. Send session.close (best-effort; ignore errors)
        if let Err(e) = self.call("session.close", serde_json::json!({})).await {
            tracing::warn!(session_id = %self.session_id, error = %e, "AWP: session.close failed (ignored)");
        } else {
            tracing::debug!(session_id = %self.session_id, "AWP: session.close OK");
        }

        // 2. Send WebSocket Close frame
        if let Err(e) = self.sink.send(Message::Close(None)).await {
            tracing::warn!(session_id = %self.session_id, error = %e, "AWP: WebSocket Close frame send failed (ignored)");
        } else {
            tracing::debug!(session_id = %self.session_id, "AWP: WebSocket Close frame sent");
        }

        // 3. Drain remaining frames until the server's Close arrives (or EOF)
        //    This completes the WebSocket closing handshake per RFC 6455.
        while let Some(msg) = self.stream.next().await {
            match msg {
                Ok(Message::Close(_)) => {
                    tracing::debug!(session_id = %self.session_id, "AWP: received server Close frame — handshake complete");
                    break;
                }
                Err(e) => {
                    tracing::debug!(session_id = %self.session_id, error = %e, "AWP: error draining after Close");
                    break;
                }
                Ok(_) => {}
            }
        }

        tracing::debug!(session_id = %self.session_id, "AWP: session closed");
    }

    pub async fn navigate(&mut self, url: &str) -> Result<Value> {
        tracing::debug!(session_id = %self.session_id, url, "AWP: navigate");
        let result = self.call("page.navigate", serde_json::json!({ "url": url })).await?;
        tracing::debug!(session_id = %self.session_id, url, "AWP: navigate OK");
        Ok(result)
    }

    /// Return the SOM from page.observe.
    /// AWP spec: page.observe returns { "som": { ... } }.
    /// This method extracts and returns the inner "som" object.
    /// Returns an error if the "som" field is absent — a missing field
    /// indicates a server-side bug or protocol mismatch and must not be
    /// silently swallowed (silent fallback would produce empty page text
    /// with no indication of failure).
    pub async fn observe(&mut self) -> Result<Value> {
        tracing::debug!(session_id = %self.session_id, "AWP: observe");
        let result = self.call("page.observe", serde_json::json!({})).await?;
        let som = extract_som_field(result)?;
        tracing::debug!(session_id = %self.session_id, "AWP: observe OK");
        Ok(som)
    }

    /// Execute a primitive action via page.act.
    ///
    /// `action`: "click" | "type" | "scroll" | "select"
    /// `target`: AWP target descriptor.
    ///   Per spec, supported target forms:
    ///     { "ref": "e_8f3a1b" }           — direct element ID from SOM
    ///     { "text": "...", "role": "..." } — semantic query
    ///     { "css": "button.primary" }      — CSS selector fallback
    ///   NOTE: use "css" key for CSS selectors, not "selector".
    /// `value`: for type/select — the text or option value
    pub async fn act(&mut self, action: &str, target: Value, value: Option<&str>) -> Result<Value> {
        tracing::debug!(session_id = %self.session_id, action, "AWP: act");
        let mut intent = serde_json::json!({ "action": action, "target": target });
        if let Some(v) = value {
            intent["value"] = Value::String(v.to_string());
        }
        let result = self.call("page.act", serde_json::json!({ "intent": intent })).await?;
        tracing::debug!(session_id = %self.session_id, action, "AWP: act OK");
        Ok(result)
    }

    /// Extract structured data from the current SOM.
    ///
    /// AWP spec: fields is a map of field-name → field-query.
    /// Supported field query types:
    ///   { "role": "heading", "level": 1 }   — by role
    ///   { "role": "link", "all": true }      — all elements of a role
    ///   { "text_match": "regex" }            — by text regex
    ///   { "ref": "e_xxx" }                   — by element ID
    ///
    /// Returns the full result object (contains "data" and "provenance" keys).
    pub async fn extract(&mut self, fields: Value) -> Result<Value> {
        tracing::debug!(session_id = %self.session_id, "AWP: extract");
        let result = self.call("page.extract", serde_json::json!({ "fields": fields })).await?;
        tracing::debug!(session_id = %self.session_id, "AWP: extract OK");
        Ok(result)
    }
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

/// Return a ready `AwpClient` or a clear error when not configured.
fn awp_client(browser_cfg: &BrowserConfig) -> Result<AwpClient> {
    let url = browser_cfg
        .awp_url
        .as_deref()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "Browser tool requires [browser] awp_url in config.toml.\n\
                 Start your AWP-compatible server and set:\n\
                 [browser]\n  awp_url = \"ws://127.0.0.1:9222\""
            )
        })?;
    Ok(AwpClient::new(url))
}

// ─── Tool implementations ─────────────────────────────────────────────────────

/// Check whether the configured AWP server is reachable.
pub async fn check_awp_server(
    _args: &Value,
    _root: &Path,
    browser_cfg: &BrowserConfig,
) -> Result<ToolResult> {
    let url = match browser_cfg.awp_url.as_deref().filter(|s| !s.is_empty()) {
        Some(u) => u.to_string(),
        None => {
            return Ok(ToolResult {
                output: "AWP server URL is not configured.\n\
                         Add the following to config.toml and start your AWP server:\n\
                         [browser]\n  awp_url = \"ws://127.0.0.1:9222\""
                    .to_string(),
                success: false,
            });
        }
    };

    let client = AwpClient::new(&url);
    tracing::debug!(url = %client.ws_url, "AWP: checking reachability");
    if client.is_reachable().await {
        tracing::debug!(url = %client.ws_url, "AWP: server reachable");
        Ok(ToolResult::ok(format!(
            "AWP server is reachable at {}",
            client.ws_url
        )))
    } else {
        tracing::debug!(url = %client.ws_url, "AWP: server not reachable");
        Ok(ToolResult {
            output: format!(
                "AWP server is not reachable at {}.\n\
                 Please start your AWP-compatible server before using browser tools.",
                client.ws_url
            ),
            success: false,
        })
    }
}

pub async fn browser_navigate(
    args: &Value,
    _root: &Path,
    browser_cfg: &BrowserConfig,
) -> Result<ToolResult> {
    let url = str_arg(args, "url")?;
    let wait_ms = read_wait_ms(args, DEFAULT_WAIT_MS);
    let client = awp_client(browser_cfg)?;
    let url_clone = url.clone();

    client
        .with_session(|session| Box::pin(async move {
            let nav_result = session
                .navigate(&url_clone)
                .await
                .with_context(|| format!("AWP page.navigate failed: {url_clone}"))?;

            // Wait for JS to settle before observing
            if wait_ms > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(wait_ms)).await;
            }

            let som = session.observe().await.context("AWP page.observe failed")?;

            // navigate result: url, status, html_bytes, som_ready, load_ms
            let nav_meta = serde_json::to_string_pretty(&nav_result).unwrap_or_default();
            let som_text = serde_json::to_string_pretty(&som).unwrap_or_else(|_| som.to_string());
            let summary = format!(
                "Navigated to: {url} (waited {wait_ms}ms)\nNavigation: {nav_meta}\nSOM: {som_text}"
            );
            Ok(ToolResult::ok(summary))
        }))
        .await
}

pub async fn browser_get_text(
    args: &Value,
    _root: &Path,
    browser_cfg: &BrowserConfig,
) -> Result<ToolResult> {
    let selector = args
        .get("selector")
        .and_then(|v| v.as_str())
        .unwrap_or("body")
        .to_string();
    let nav_url = args.get("url").and_then(|v| v.as_str()).map(str::to_string);
    let wait_ms = read_wait_ms(args, DEFAULT_WAIT_MS);
    let client = awp_client(browser_cfg)?;

    client
        .with_session(|session| Box::pin(async move {
            if let Some(url) = nav_url {
                session
                    .navigate(&url)
                    .await
                    .with_context(|| format!("AWP page.navigate failed: {url}"))?;
                if wait_ms > 0 {
                    tokio::time::sleep(std::time::Duration::from_millis(wait_ms)).await;
                }
            }

            // Build AWP-compatible field queries.
            // "body" or unknown selectors → get all visible text via full SOM observe.
            // Known AWP roles → use role query.
            // Otherwise → use text_match.
            if selector == "body" || selector.is_empty() {
                let som = session.observe().await.context("AWP page.observe failed")?;
                let text = extract_text_from_som(&som);
                return Ok(ToolResult::ok(format!("Page text:\n{text}")));
            }

            let awp_roles = [
                "link", "button", "text_input", "textarea", "select", "checkbox",
                "radio", "heading", "image", "list", "table", "paragraph", "section",
            ];
            let fields = if awp_roles.contains(&selector.as_str()) {
                serde_json::json!({
                    "content": { "role": selector, "all": true, "props": ["text"] }
                })
            } else {
                serde_json::json!({
                    "content": { "text_match": selector, "all": true }
                })
            };
            let extract_result = session.extract(fields).await.context("AWP page.extract failed")?;

            let text = extract_result
                .get("data")
                .and_then(|d| d.get("content"))
                .map(|v| format_extract_value(v))
                .unwrap_or_default();
            Ok(ToolResult::ok(format!("Text from selector '{selector}': {text}")))
        }))
        .await
}

pub async fn browser_screenshot(
    args: &Value,
    root: &Path,
    browser_cfg: &BrowserConfig,
) -> Result<ToolResult> {
    let rel_path = args
        .get("path")
        .and_then(|v| v.as_str())
        .unwrap_or("screenshot.png");
    let full_path = resolve(root, rel_path)?;
    let nav_url = args.get("url").and_then(|v| v.as_str()).map(str::to_string);
    let wait_ms = read_wait_ms(args, DEFAULT_WAIT_MS);
    let client = awp_client(browser_cfg)?;

    client
        .with_session(|session| Box::pin(async move {
            if let Some(url) = nav_url {
                session
                    .navigate(&url)
                    .await
                    .with_context(|| format!("AWP page.navigate failed: {url}"))?;
                if wait_ms > 0 {
                    tokio::time::sleep(std::time::Duration::from_millis(wait_ms)).await;
                }
            }

            let som = session.observe().await.context("AWP page.observe failed")?;

            // AWP v0.1 does not have a screenshot method (deferred to v0.2).
            // Save the SOM snapshot as JSON instead — still useful for debugging.
            let som_path = full_path.with_extension("som.json");
            if let Some(parent) = som_path.parent() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("Cannot create dir: {}", parent.display()))?;
            }
            let som_text = serde_json::to_string_pretty(&som).unwrap_or_else(|_| som.to_string());
            std::fs::write(&som_path, &som_text)
                .with_context(|| format!("Cannot write SOM to: {}", som_path.display()))?;

            Ok(ToolResult::ok(format!(
                "AWP v0.1: page.screenshot deferred to v0.2. \
                 SOM snapshot saved to: {}",
                som_path.display()
            )))
        }))
        .await
}

pub async fn browser_action(
    args: &Value,
    _root: &Path,
    browser_cfg: &BrowserConfig,
) -> Result<ToolResult> {
    let action = str_arg(args, "action")?;

    let valid_actions = ["click", "type", "scroll", "select", "hover", "clear"];
    if !valid_actions.contains(&action.as_str()) {
        bail!("Unknown action: {action}. Valid: {}", valid_actions.join(", "));
    }

    // Target: prefer "ref" (element ID from SOM) then "css" (CSS selector).
    // "selector" is accepted as alias for "css" for backwards compatibility.
    // NOTE: AWP SOM only contains elements with ARIA roles. Elements without
    // a role (plain div, td, span) are NOT visible in SOM. Use css= target
    // for such elements, e.g. css="div.cell[data-index='0']".
    //
    // IMPORTANT: url is required — AWP sessions are stateless (one per call).
    let element_ref = args.get("ref").and_then(|v| v.as_str()).map(str::to_string);
    let css_selector = args
        .get("css")
        .or_else(|| args.get("selector"))
        .and_then(|v| v.as_str())
        .unwrap_or("body")
        .to_string();

    let nav_url = args.get("url").and_then(|v| v.as_str()).map(str::to_string);

    // Require url: AWP sessions are stateless (one connection per tool call).
    // Without url there is no page loaded in the new session and page.act will
    // fail with NOT_FOUND. Catch this early to give a clear diagnostic.
    if nav_url.is_none() {
        bail!(
            "browser_action requires a 'url' argument — each tool call opens a new \
             AWP session with no page loaded. Pass the URL of the page to interact with, e.g. \
             browser_action(action=\"click\", url=\"http://localhost:9000\", css=\"div.cell[data-index='0']\")"
        );
    }

    let text_value = args
        .get("text")
        .or_else(|| args.get("value"))
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let option_value = args
        .get("option")
        .or_else(|| args.get("value"))
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let direction = args
        .get("direction")
        .and_then(|v| v.as_str())
        .unwrap_or("down")
        .to_string();
    let amount = args.get("amount").and_then(|v| v.as_i64()).unwrap_or(300);
    let wait_ms = read_wait_ms(args, 0);
    let client = awp_client(browser_cfg)?;

    client
        .with_session(|session| Box::pin(async move {
            if let Some(url) = nav_url {
                session
                    .navigate(&url)
                    .await
                    .with_context(|| format!("AWP page.navigate failed: {url}"))?;
                tokio::time::sleep(std::time::Duration::from_millis(DEFAULT_WAIT_MS)).await;
            }

            // Build the AWP target descriptor per spec section 4.6:
            //   { "ref": "e_xxx" }         — preferred (stable element ID from SOM)
            //   { "css": "button.primary" } — CSS selector fallback
            let target = if let Some(r) = element_ref {
                serde_json::json!({ "ref": r })
            } else {
                serde_json::json!({ "css": css_selector })
            };
            let target_label = target
                .get("ref")
                .or_else(|| target.get("css"))
                .and_then(|v| v.as_str())
                .unwrap_or("?")
                .to_string();

            let msg = match action.as_str() {
                "click" => {
                    session
                        .act("click", target, None)
                        .await
                        .with_context(|| format!("AWP click failed on '{target_label}'"))?;
                    format!("Clicked: {target_label}")
                }
                "type" => {
                    let text = text_value.unwrap_or_default();
                    session
                        .act("type", target, Some(&text))
                        .await
                        .with_context(|| format!("AWP type failed on '{target_label}'"))?;
                    format!("Typed '{text}' into {target_label}")
                }
                "scroll" => {
                    let scroll_target = serde_json::json!({
                        "direction": direction,
                        "amount": amount
                    });
                    session
                        .act("scroll", scroll_target, None)
                        .await
                        .context("AWP scroll failed")?;
                    format!("Scrolled {direction} by {amount}")
                }
                "select" => {
                    let option = option_value.unwrap_or_default();
                    session
                        .act("select", target, Some(&option))
                        .await
                        .with_context(|| format!("AWP select failed on '{target_label}'"))?;
                    format!("Selected '{option}' in {target_label}")
                }
                // hover/clear: AWP v0.1 has no native equivalents — emulated.
                "hover" => {
                    session
                        .act("click", target, None)
                        .await
                        .with_context(|| format!("AWP hover (via click) failed on '{target_label}'"))?;
                    format!("Hovered (via click) over {target_label}")
                }
                "clear" => {
                    session
                        .act("type", target, Some(""))
                        .await
                        .with_context(|| format!("AWP clear (via type \"\") failed on '{target_label}'"))?;
                    format!("Cleared {target_label}")
                }
                _ => unreachable!("validated above"),
            };

            if wait_ms > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(wait_ms)).await;
            }

            Ok(ToolResult::ok(msg))
        }))
        .await
}

// ─── Internal helpers ─────────────────────────────────────────────────────────

/// Open a session: send awp.hello, then session.create.
/// Returns an AwpSession with the session_id set.
async fn awp_open_session(mut sink: WsSink, mut stream: WsStream) -> Result<AwpSession> {
    // 1. awp.hello
    tracing::debug!("AWP: sending hello");
    send_and_receive(&mut sink, &mut stream, "awp.hello", serde_json::json!({
        "client_name":    "do_it",
        "client_version": "0.1.0",
        "awp_version":    "0.1"
    }))
    .await
    .context("AWP: hello failed")?;
    tracing::debug!("AWP: hello OK");

    // 2. session.create
    tracing::debug!("AWP: creating session");
    let session_result = send_and_receive(
        &mut sink,
        &mut stream,
        "session.create",
        serde_json::json!({}),
    )
    .await
    .context("AWP: session.create failed")?;

    let session_id = session_result
        .get("session_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("AWP: session.create missing session_id in result"))?
        .to_string();
    tracing::debug!(session_id = %session_id, "AWP: session.create OK");

    Ok(AwpSession { session_id, sink, stream })
}

/// Send one AWP request and wait for the matching response.
async fn send_and_receive(
    sink: &mut WsSink,
    stream: &mut WsStream,
    method: &str,
    params: Value,
) -> Result<Value> {
    let id = next_id();
    let req = AwpRequest {
        id: id.clone(),
        msg_type: "request",
        method,
        params,
    };
    let frame = serde_json::to_string(&req).expect("serialize awp request");
    sink.send(Message::Text(frame.into()))
        .await
        .with_context(|| format!("AWP: send {method} failed"))?;

    loop {
        match stream.next().await {
            None => bail!("AWP: connection closed waiting for {method} response"),
            Some(Err(e)) => bail!("AWP: WebSocket error during {method}: {e}"),
            Some(Ok(Message::Text(raw))) => {
                let resp: AwpResponse = serde_json::from_str(&raw)
                    .with_context(|| format!("AWP: non-JSON response to {method}: {raw}"))?;
                if resp.id.as_deref() != Some(&id) {
                    continue; // skip events and unrelated responses
                }
                if let Some(err) = resp.error {
                    bail!("AWP [{method}] error [{}]: {}", err.code, err.message);
                }
                return Ok(resp.result.unwrap_or(Value::Null));
            }
            Some(Ok(Message::Close(_))) => {
                bail!("AWP: server closed connection while waiting for {method} response");
            }
            Some(Ok(_)) => {} // ping / pong / binary — ignore
        }
    }
}

/// Extract the `"som"` field from a `page.observe` response.
///
/// AWP spec §4.5: the server MUST return `{ "som": { ... } }`.
/// Returns an explicit error when the field is absent so callers get a
/// clear diagnostic instead of silently receiving empty page text.
fn extract_som_field(result: Value) -> Result<Value> {
    result
        .get("som")
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("AWP: page.observe response missing 'som' field"))
}

/// Extract plain text from a SOM snapshot (page.observe result).
/// Concatenates text fields from all elements across all regions.
fn extract_text_from_som(som: &Value) -> String {
    let mut parts: Vec<String> = Vec::new();

    fn visit(node: &Value, parts: &mut Vec<String>) {
        if let Some(text) = node.get("text").and_then(|v| v.as_str()) {
            if !text.trim().is_empty() {
                parts.push(text.to_string());
            }
        }
        if let Some(elements) = node.get("elements").and_then(|v| v.as_array()) {
            for el in elements {
                visit(el, parts);
            }
        }
        if let Some(children) = node.get("children").and_then(|v| v.as_array()) {
            for child in children {
                visit(child, parts);
            }
        }
        if let Some(regions) = node.get("regions").and_then(|v| v.as_array()) {
            for region in regions {
                visit(region, parts);
            }
        }
    }

    visit(som, &mut parts);
    parts.join("\n")
}

/// Format an extract result value (single item or array) as a string.
fn format_extract_value(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Array(items) => items
            .iter()
            .filter_map(|item| {
                item.get("text")
                    .and_then(|t| t.as_str())
                    .map(str::to_string)
                    .or_else(|| {
                        if item.is_string() {
                            item.as_str().map(str::to_string)
                        } else {
                            None
                        }
                    })
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => serde_json::to_string(v).unwrap_or_default(),
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config_struct::BrowserConfig;
    use serde_json::json;
    use tempfile::TempDir;

    fn no_browser() -> BrowserConfig {
        BrowserConfig::default()
    }

    fn with_awp(url: &str) -> BrowserConfig {
        BrowserConfig {
            awp_url: Some(url.to_string()),
            ..BrowserConfig::default()
        }
    }

    // ── wait_ms helper ─────────────────────────────────────────────────────

    #[test]
    fn read_wait_ms_defaults_when_absent() {
        let args = json!({});
        assert_eq!(read_wait_ms(&args, 500), 500);
    }

    #[test]
    fn read_wait_ms_reads_explicit_value() {
        let args = json!({ "wait_ms": 1200 });
        assert_eq!(read_wait_ms(&args, 500), 1200);
    }

    #[test]
    fn read_wait_ms_clamps_to_max() {
        let args = json!({ "wait_ms": 999_999 });
        assert_eq!(read_wait_ms(&args, 500), MAX_WAIT_MS);
    }

    #[test]
    fn read_wait_ms_zero_is_valid() {
        let args = json!({ "wait_ms": 0 });
        assert_eq!(read_wait_ms(&args, 500), 0);
    }

    // ── Arg validation (no live server needed) ─────────────────────────────

    #[tokio::test]
    async fn navigate_requires_url_arg() {
        let tmp = TempDir::new().unwrap();
        let err = browser_navigate(&json!({}), tmp.path(), &with_awp("ws://127.0.0.1:19999"))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("Missing arg: url"), "{err}");
    }

    #[tokio::test]
    async fn action_requires_action_arg() {
        let tmp = TempDir::new().unwrap();
        let err = browser_action(&json!({}), tmp.path(), &with_awp("ws://127.0.0.1:19999"))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("Missing arg: action"), "{err}");
    }

    #[tokio::test]
    async fn action_rejects_unknown_action() {
        let tmp = TempDir::new().unwrap();
        let err = browser_action(
            &json!({ "action": "teleport" }),
            tmp.path(),
            &with_awp("ws://127.0.0.1:19999"),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("Unknown action: teleport"), "{err}");
    }

    #[tokio::test]
    async fn action_rejects_missing_url() {
        let tmp = TempDir::new().unwrap();
        let err = browser_action(
            &json!({ "action": "click", "css": "div.cell" }),
            tmp.path(),
            &with_awp("ws://127.0.0.1:19999"),
        )
        .await
        .unwrap_err();
        // Must fail before connecting to AWP — clear diagnostic about url requirement.
        assert!(
            err.to_string().contains("url"),
            "expected url-missing diagnostic, got: {err}"
        );
        assert!(
            err.to_string().contains("REQUIRED") || err.to_string().contains("requires"),
            "expected requirement wording, got: {err}"
        );
    }

    #[tokio::test]
    async fn screenshot_rejects_path_traversal() {
        let tmp = TempDir::new().unwrap();
        let err = browser_screenshot(
            &json!({ "path": "../outside/shot.png" }),
            tmp.path(),
            &with_awp("ws://127.0.0.1:19999"),
        )
        .await
        .unwrap_err();
        assert!(!err.to_string().is_empty());
    }

    // ── No-config error ────────────────────────────────────────────────────

    #[tokio::test]
    async fn navigate_errors_without_awp_url() {
        let tmp = TempDir::new().unwrap();
        let err =
            browser_navigate(&json!({ "url": "http://example.com" }), tmp.path(), &no_browser())
                .await
                .unwrap_err();
        assert!(err.to_string().contains("awp_url"), "{err}");
    }

    // ── check_awp_server ───────────────────────────────────────────────────

    #[tokio::test]
    async fn check_awp_server_reports_missing_config() {
        let tmp = TempDir::new().unwrap();
        let result = check_awp_server(&json!({}), tmp.path(), &no_browser())
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.output.contains("awp_url"), "{}", result.output);
    }

    #[tokio::test]
    async fn check_awp_server_reports_unreachable() {
        let tmp = TempDir::new().unwrap();
        let cfg = BrowserConfig {
            awp_url: Some("ws://127.0.0.1:19997".to_string()),
            ..BrowserConfig::default()
        };
        let result = check_awp_server(&json!({}), tmp.path(), &cfg).await.unwrap();
        assert!(!result.success, "expected failure, got: {}", result.output);
        assert!(result.output.contains("not reachable"), "{}", result.output);
        assert!(!result.output.to_lowercase().contains("plasmate"), "{}", result.output);
    }

    // ── URL normalisation ──────────────────────────────────────────────────

    #[test]
    fn awp_client_normalises_http_to_ws() {
        let c = AwpClient::new("http://127.0.0.1:9222");
        assert!(c.ws_url.starts_with("ws://"), "got: {}", c.ws_url);
    }

    #[test]
    fn awp_client_normalises_https_to_wss() {
        let c = AwpClient::new("https://example.com:9222");
        assert!(c.ws_url.starts_with("wss://"), "got: {}", c.ws_url);
    }

    #[test]
    fn awp_client_keeps_ws_url() {
        let c = AwpClient::new("ws://127.0.0.1:9222");
        assert!(c.ws_url.starts_with("ws://"), "got: {}", c.ws_url);
    }

    #[test]
    fn awp_client_appends_trailing_slash() {
        let c = AwpClient::new("ws://127.0.0.1:9222");
        assert!(c.ws_url.ends_with('/'), "got: {}", c.ws_url);
    }

    // ── AWP envelope serialisation ─────────────────────────────────────────

    #[test]
    fn awp_request_has_correct_envelope() {
        let req = AwpRequest {
            id: "42".to_string(),
            msg_type: "request",
            method: "page.navigate",
            params: json!({ "session_id": "s_1", "url": "https://example.com" }),
        };
        let v = serde_json::to_value(&req).unwrap();
        assert_eq!(v["id"], "42");
        assert_eq!(v["type"], "request");
        assert_eq!(v["method"], "page.navigate");
        assert_eq!(v["params"]["url"], "https://example.com");
        assert!(v.get("jsonrpc").is_none());
    }

    // ── SOM text extraction ────────────────────────────────────────────────

    #[test]
    fn extract_text_from_som_collects_nested_text() {
        let som = json!({
            "regions": [
                {
                    "role": "navigation",
                    "elements": [
                        { "id": "e1", "role": "link", "text": "Home" },
                        { "id": "e2", "role": "link", "text": "About" }
                    ]
                },
                {
                    "role": "main",
                    "elements": [
                        { "id": "e3", "role": "heading", "text": "Welcome" },
                        {
                            "id": "e4",
                            "role": "paragraph",
                            "text": "Hello world",
                            "children": [
                                { "id": "e5", "role": "link", "text": "click here" }
                            ]
                        }
                    ]
                }
            ]
        });
        let text = extract_text_from_som(&som);
        assert!(text.contains("Home"), "must contain 'Home'");
        assert!(text.contains("About"), "must contain 'About'");
        assert!(text.contains("Welcome"), "must contain 'Welcome'");
        assert!(text.contains("Hello world"), "must contain 'Hello world'");
        assert!(text.contains("click here"), "must contain 'click here'");
    }

    // ── extract_som_field ──────────────────────────────────────────────────

    #[test]
    fn extract_som_field_returns_som_when_present() {
        let result = json!({ "som": { "regions": [] }, "meta": "ignored" });
        let som = extract_som_field(result).unwrap();
        assert_eq!(som, json!({ "regions": [] }));
    }

    #[test]
    fn extract_som_field_errors_when_som_absent() {
        let result = json!({ "status": "ok" });
        let err = extract_som_field(result).unwrap_err();
        assert!(
            err.to_string().contains("missing 'som' field"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn extract_text_from_som_ignores_empty_text() {
        let som = json!({
            "regions": [
                {
                    "elements": [
                        { "id": "e1", "role": "separator" },
                        { "id": "e2", "role": "link", "text": "  " },
                        { "id": "e3", "role": "link", "text": "Real link" }
                    ]
                }
            ]
        });
        let text = extract_text_from_som(&som);
        assert!(text.contains("Real link"));
        // empty/whitespace-only text must not produce blank lines in output
        let lines: Vec<&str> = text.lines().filter(|l| l.trim().is_empty()).collect();
        assert!(lines.is_empty(), "no blank lines expected: {:?}", text);
    }
}
