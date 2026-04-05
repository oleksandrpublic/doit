//! LLM backend abstraction.
//!
//! Supports three wire protocols:
//!   - Ollama  (`/api/chat`)
//!   - OpenAI  (`/v1/chat/completions`) — compatible with any OpenAI-API service
//!   - Anthropic (`/v1/messages`) — compatible with any Anthropic-API service
//!
//! The top-level entry point is [`LlmClient`]. Per-role routing is handled
//! by the caller (config_struct / agent): each role receives its own
//! `BackendConfig` and creates or borrows an `LlmClient` from it.
//!
//! `OllamaClient` is kept as a thin public alias so existing call-sites
//! that import it by name continue to compile without change.

use anyhow::{Context, Result, anyhow, bail};
use serde_json::Value;
use std::time::Duration;

// ──────────────────────────────────────────────────────────────
// Public types
// ──────────────────────────────────────────────────────────────

/// Wire protocol to use when talking to the LLM service.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum BackendKind {
    /// Ollama — `POST /api/chat`
    #[default]
    Ollama,
    /// Any OpenAI-compatible endpoint — `POST /v1/chat/completions`
    OpenAI,
    /// Any Anthropic-compatible endpoint — `POST /v1/messages`
    Anthropic,
}

/// Everything needed to talk to one LLM service for one role.
///
/// Construct from `config.toml` defaults or per-role `[roles.<name>]` table.
///
/// ```toml
/// # global default — Ollama, no key needed
/// llm_url     = "http://localhost:11434"
/// llm_backend = "ollama"
/// model       = "qwen3.5:cloud"
///
/// # per-role override — use a remote OpenAI-compatible service
/// [roles.boss]
/// llm_url     = "https://api.minimax.io/v1"
/// llm_backend = "openai"
/// llm_api_key = "sk-…"
/// model       = "abab6.5s-chat"
/// ```
#[derive(Debug, Clone)]
pub struct BackendConfig {
    pub url: String,
    pub kind: BackendKind,
    /// `None` = no Authorization header (Ollama default, local services).
    pub api_key: Option<String>,
    pub model: String,
    pub temperature: f64,
    pub max_tokens: u32,
}

impl BackendConfig {
    /// Convenience constructor that mirrors the original `OllamaClient::new` signature
    /// so existing code can migrate incrementally.
    pub fn ollama(base_url: &str, model: &str, temperature: f64, max_tokens: u32) -> Self {
        Self {
            url: base_url.trim_end_matches('/').to_string(),
            kind: BackendKind::Ollama,
            api_key: None,
            model: model.to_string(),
            temperature,
            max_tokens,
        }
    }

    pub fn openai(
        base_url: &str,
        api_key: Option<String>,
        model: &str,
        temperature: f64,
        max_tokens: u32,
    ) -> Self {
        Self {
            url: base_url.trim_end_matches('/').to_string(),
            kind: BackendKind::OpenAI,
            api_key,
            model: model.to_string(),
            temperature,
            max_tokens,
        }
    }

    pub fn anthropic(
        base_url: &str,
        api_key: Option<String>,
        model: &str,
        temperature: f64,
        max_tokens: u32,
    ) -> Self {
        Self {
            url: base_url.trim_end_matches('/').to_string(),
            kind: BackendKind::Anthropic,
            api_key,
            model: model.to_string(),
            temperature,
            max_tokens,
        }
    }
}

// ──────────────────────────────────────────────────────────────
// LlmClient — unified chat interface
// ──────────────────────────────────────────────────────────────

/// Unified LLM client. Create one per backend config (typically one per role,
/// or share if multiple roles use the same backend).
pub struct LlmClient {
    cfg: BackendConfig,
    http: reqwest::Client,
}

impl LlmClient {
    pub fn new(cfg: BackendConfig) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(600))
            .connect_timeout(Duration::from_secs(10))
            .build()
            .unwrap_or_default();
        Self { cfg, http }
    }

    /// Single-turn chat.
    pub async fn chat(&self, system: &str, user: &str) -> Result<String> {
        self.chat_inner(system, user, None).await
    }

    /// Vision chat — attaches an image as base64 alongside the text prompt.
    /// Only supported by backends / models that accept image input.
    pub async fn chat_with_image(
        &self,
        system: &str,
        user: &str,
        image_path: &std::path::Path,
    ) -> Result<String> {
        use base64::{Engine, engine::general_purpose::STANDARD};

        let bytes = std::fs::read(image_path)
            .map_err(|e| anyhow!("Failed to read image '{}': {e}", image_path.display()))?;
        let b64 = STANDARD.encode(&bytes);
        self.chat_inner(system, user, Some(b64)).await
    }

    async fn chat_inner(
        &self,
        system: &str,
        user: &str,
        image_b64: Option<String>,
    ) -> Result<String> {
        match self.cfg.kind {
            BackendKind::Ollama => self.ollama_chat(system, user, image_b64).await,
            BackendKind::OpenAI => self.openai_chat(system, user, image_b64).await,
            BackendKind::Anthropic => self.anthropic_chat(system, user, image_b64).await,
        }
    }

    // ── Ollama ──────────────────────────────────────────────

    async fn ollama_chat(
        &self,
        system: &str,
        user: &str,
        image_b64: Option<String>,
    ) -> Result<String> {
        let user_msg = if let Some(b64) = image_b64 {
            serde_json::json!({ "role": "user", "content": user, "images": [b64] })
        } else {
            serde_json::json!({ "role": "user", "content": user })
        };

        let body = serde_json::json!({
            "model": self.cfg.model,
            "stream": false,
            "options": {
                "temperature": self.cfg.temperature,
                "num_predict": self.cfg.max_tokens,
            },
            "messages": [
                { "role": "system", "content": system },
                user_msg,
            ]
        });

        let url = format!("{}/api/chat", self.cfg.url);
        let resp = self.post_json(&url, &body).await?;
        extract_string(&resp, &["message", "content"])
    }

    // ── OpenAI-compatible ───────────────────────────────────

    async fn openai_chat(
        &self,
        system: &str,
        user: &str,
        image_b64: Option<String>,
    ) -> Result<String> {
        let user_content: Value = if let Some(b64) = image_b64 {
            serde_json::json!([
                { "type": "text", "text": user },
                {
                    "type": "image_url",
                    "image_url": {
                        "url": format!("data:image/jpeg;base64,{b64}")
                    }
                }
            ])
        } else {
            Value::String(user.to_string())
        };

        let body = serde_json::json!({
            "model": self.cfg.model,
            "temperature": self.cfg.temperature,
            "max_tokens": self.cfg.max_tokens,
            "stream": false,
            "messages": [
                { "role": "system", "content": system },
                { "role": "user", "content": user_content }
            ]
        });

        let url = format!("{}/v1/chat/completions", self.cfg.url);
        let resp = self.post_json(&url, &body).await?;
        extract_openai_text(&resp)
    }

    // ── Anthropic-compatible ────────────────────────────────

    async fn anthropic_chat(
        &self,
        system: &str,
        user: &str,
        image_b64: Option<String>,
    ) -> Result<String> {
        let user_content: Value = if let Some(b64) = image_b64 {
            serde_json::json!([
                {
                    "type": "image",
                    "source": {
                        "type": "base64",
                        "media_type": "image/jpeg",
                        "data": b64
                    }
                },
                { "type": "text", "text": user }
            ])
        } else {
            Value::String(user.to_string())
        };

        let body = serde_json::json!({
            "model": self.cfg.model,
            "system": system,
            "max_tokens": self.cfg.max_tokens,
            "temperature": self.cfg.temperature,
            "stream": false,
            "messages": [
                { "role": "user", "content": user_content }
            ]
        });

        let url = format!("{}/v1/messages", self.cfg.url);

        let mut req = self
            .http
            .post(&url)
            .header(reqwest::header::ACCEPT, "application/json")
            .json(&body);

        req = req.header("anthropic-version", "2023-06-01");
        if let Some(key) = &self.cfg.api_key {
            req = req.header("x-api-key", key);
        }

        let resp = send_and_parse(req, &self.cfg.url).await?;
        extract_anthropic_text(&resp)
    }

    // ── shared HTTP helper ──────────────────────────────────

    /// POST JSON body, attach Bearer token if configured, return parsed JSON.
    async fn post_json(&self, url: &str, body: &Value) -> Result<Value> {
        let mut req = self
            .http
            .post(url)
            .header(reqwest::header::ACCEPT, "application/json")
            .json(body);

        if let Some(key) = &self.cfg.api_key {
            req = req.header(reqwest::header::AUTHORIZATION, format!("Bearer {key}"));
        }

        send_and_parse(req, &self.cfg.url).await
    }

    // ── model override ──────────────────────────────────────

    /// Return a new `LlmClient` with the model field overridden for one call.
    /// Used when the agent re-routes to a different model mid-step.
    pub(crate) fn with_model(&self, model: &str) -> LlmClient {
        let mut cfg = self.cfg.clone();
        cfg.model = model.to_string();
        LlmClient::new(cfg)
    }

    // ── model availability check ────────────────────────────

    /// Verify that models are reachable. Only meaningful for Ollama (which
    /// exposes `/api/tags`). For other backends this is a no-op that logs a
    /// single info line.
    pub async fn check_models(&self, models: &[&str]) -> Result<()> {
        match self.cfg.kind {
            BackendKind::Ollama => self.ollama_check_models(models).await,
            BackendKind::OpenAI | BackendKind::Anthropic => {
                tracing::info!(
                    backend = ?self.cfg.kind,
                    url = %self.cfg.url,
                    "Model availability check skipped (non-Ollama backend)"
                );
                Ok(())
            }
        }
    }

    async fn ollama_check_models(&self, models: &[&str]) -> Result<()> {
        let url = format!("{}/api/tags", self.cfg.url);
        let resp = self
            .http
            .get(&url)
            .header(reqwest::header::ACCEPT, "application/json")
            .send()
            .await
            .map_err(|e| anyhow!("Cannot reach Ollama at {}: {e}", self.cfg.url))?;

        let status = resp.status();
        let text = resp
            .text()
            .await
            .context("Failed to read Ollama model list response body")?;

        if !status.is_success() {
            bail!(
                "Ollama returned HTTP {status}: {}",
                preview_text(&text, 1200)
            );
        }

        let json: Value = serde_json::from_str(&text).map_err(|e| {
            anyhow!(
                "Failed to decode Ollama model list as JSON: {e}. Body preview: {}",
                preview_text(&text, 1200)
            )
        })?;

        let available: Vec<String> = json
            .get("models")
            .and_then(|m| m.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|m| {
                        m.get("name")
                            .and_then(|n| n.as_str())
                            .map(|s| s.to_string())
                    })
                    .collect()
            })
            .unwrap_or_default();

        for model in models {
            if available.iter().any(|a| a.starts_with(model)) {
                tracing::info!("Model '{}' ✓", model);
            } else {
                tracing::warn!("Model '{}' not found — run: ollama pull {}", model, model);
            }
        }

        Ok(())
    }
}

// ──────────────────────────────────────────────────────────────
// OllamaClient — backward-compatible shim
// ──────────────────────────────────────────────────────────────

/// Backward-compatible wrapper. Existing call-sites that use
/// `OllamaClient::new` / `.chat` / `.chat_with_image` / `.check_models`
/// keep working without change.
///
/// New code should use [`LlmClient`] with an appropriate [`BackendConfig`].
pub struct OllamaClient(LlmClient);

impl OllamaClient {
    pub fn new(base_url: &str, temperature: f64, max_tokens: u32) -> Self {
        let cfg = BackendConfig::ollama(base_url, "__placeholder__", temperature, max_tokens);
        Self(LlmClient::new(cfg))
    }

    pub async fn chat(&self, model: &str, system: &str, user: &str) -> Result<String> {
        self.with_model(model).chat(system, user).await
    }

    pub async fn chat_with_image(
        &self,
        model: &str,
        system: &str,
        user: &str,
        image_path: &std::path::Path,
    ) -> Result<String> {
        self.with_model(model)
            .chat_with_image(system, user, image_path)
            .await
    }

    pub async fn check_models(&self, models: &[&str]) -> Result<()> {
        self.0.check_models(models).await
    }

    /// Return an `LlmClient` with the model field overridden for one call.
    fn with_model(&self, model: &str) -> LlmClient {
        let mut cfg = self.0.cfg.clone();
        cfg.model = model.to_string();
        LlmClient::new(cfg)
    }
}

// ──────────────────────────────────────────────────────────────
// Internal helpers
// ──────────────────────────────────────────────────────────────

async fn send_and_parse(req: reqwest::RequestBuilder, base_url: &str) -> Result<Value> {
    let resp = req
        .send()
        .await
        .map_err(|e| anyhow!("LLM request failed: {e}\nIs the service running at {base_url}?"))?;

    let status = resp.status();
    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("<unknown>")
        .to_string();

    let body = resp
        .text()
        .await
        .with_context(|| format!("Failed to read LLM response body from {base_url}"))?;

    if !status.is_success() {
        bail!(
            "LLM returned HTTP {} (content-type: {}): {}",
            status,
            content_type,
            preview_text(&body, 1600)
        );
    }

    if body.trim().is_empty() {
        bail!(
            "LLM returned an empty body (content-type: {}) from {}",
            content_type,
            base_url
        );
    }

    match serde_json::from_str::<Value>(&body) {
        Ok(json) => Ok(json),
        Err(primary_err) => {
            if let Some(json) = parse_sse_json(&body) {
                tracing::warn!(
                    "LLM returned SSE-like body while JSON was expected; recovered by parsing last data frame"
                );
                return Ok(json);
            }

            bail!(
                "LLM response decode error: {} (content-type: {}). Body preview: {}",
                primary_err,
                content_type,
                preview_text(&body, 1600)
            );
        }
    }
}

/// Walk a chain of JSON keys and extract the final string value.
fn extract_string(json: &Value, path: &[&str]) -> Result<String> {
    let mut cur = json;
    for key in path {
        cur = cur
            .get(key)
            .ok_or_else(|| anyhow!("Missing key '{key}' in response: {json}"))?;
    }

    cur.as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| anyhow!("Expected string at path {:?} in: {json}", path))
}

fn extract_openai_text(json: &Value) -> Result<String> {
    if let Some(error) = json.get("error") {
        bail!("OpenAI-compatible API returned an error object: {error}");
    }

    if let Some(text) = json
        .pointer("/choices/0/message/content")
        .and_then(content_to_text)
        .filter(|s| !s.trim().is_empty())
    {
        return Ok(text);
    }

    if let Some(text) = json
        .pointer("/choices/0/delta/content")
        .and_then(content_to_text)
        .filter(|s| !s.trim().is_empty())
    {
        return Ok(text);
    }

    if let Some(text) = json
        .pointer("/choices/0/text")
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
    {
        return Ok(text.to_string());
    }

    if let Some(text) = json
        .get("output_text")
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
    {
        return Ok(text.to_string());
    }

    if let Some(text) = json
        .get("output")
        .and_then(extract_responses_output_text)
        .filter(|s| !s.trim().is_empty())
    {
        return Ok(text);
    }

    bail!("Unexpected OpenAI-compatible response shape: {json}");
}

fn extract_anthropic_text(json: &Value) -> Result<String> {
    if let Some(error) = json.get("error") {
        bail!("Anthropic-compatible API returned an error object: {error}");
    }

    if let Some(text) = json
        .get("content")
        .and_then(content_to_text)
        .filter(|s| !s.trim().is_empty())
    {
        return Ok(text);
    }

    bail!("Unexpected Anthropic response shape: {json}");
}

fn extract_responses_output_text(output: &Value) -> Option<String> {
    let items = output.as_array()?;
    let mut parts = Vec::new();

    for item in items {
        if let Some(text) = item.get("text").and_then(Value::as_str) {
            if !text.trim().is_empty() {
                parts.push(text.to_string());
            }
        }

        if let Some(content) = item.get("content").and_then(content_to_text) {
            if !content.trim().is_empty() {
                parts.push(content);
            }
        }
    }

    if parts.is_empty() {
        None
    } else {
        Some(parts.join("\n"))
    }
}

fn content_to_text(value: &Value) -> Option<String> {
    match value {
        Value::String(s) => Some(s.to_string()),
        Value::Array(items) => {
            let mut parts = Vec::new();

            for item in items {
                match item {
                    Value::String(s) => {
                        if !s.trim().is_empty() {
                            parts.push(s.to_string());
                        }
                    }
                    Value::Object(_) => {
                        if let Some(text) = item.get("text").and_then(Value::as_str) {
                            if !text.trim().is_empty() {
                                parts.push(text.to_string());
                            }
                        } else if let Some(text) = item.get("content").and_then(Value::as_str) {
                            if !text.trim().is_empty() {
                                parts.push(text.to_string());
                            }
                        }
                    }
                    _ => {}
                }
            }

            if parts.is_empty() {
                None
            } else {
                Some(parts.join("\n"))
            }
        }
        Value::Object(map) => map
            .get("text")
            .and_then(Value::as_str)
            .map(|s| s.to_string()),
        _ => None,
    }
}

fn parse_sse_json(body: &str) -> Option<Value> {
    let mut last = None;

    for line in body.lines() {
        let trimmed = line.trim();
        let Some(data) = trimmed.strip_prefix("data:") else {
            continue;
        };

        let payload = data.trim();
        if payload.is_empty() || payload == "[DONE]" {
            continue;
        }

        if let Ok(json) = serde_json::from_str::<Value>(payload) {
            last = Some(json);
        }
    }

    last
}

fn preview_text(text: &str, max_chars: usize) -> String {
    let mut out = String::new();

    for ch in text.chars().take(max_chars) {
        match ch {
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            _ => out.push(ch),
        }
    }

    if text.chars().count() > max_chars {
        out.push_str("…");
    }

    out
}
