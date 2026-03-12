use anyhow::{bail, Result};
use serde_json::Value;

pub struct OllamaClient {
    base_url: String,
    temperature: f64,
    max_tokens: u32,
    http: reqwest::Client,
}

impl OllamaClient {
    pub fn new(base_url: &str, temperature: f64, max_tokens: u32) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            temperature,
            max_tokens,
            http: reqwest::Client::new(),
        }
    }

    /// Single-turn chat. `model` is passed per-call to support per-role routing.
    pub async fn chat(&self, model: &str, system: &str, user: &str) -> Result<String> {
        self.chat_inner(model, system, user, None).await
    }

    /// Vision chat — sends an image as base64 alongside the text prompt.
    /// Requires a vision-capable model (e.g. qwen3.5:9b via llama.cpp).
    pub async fn chat_with_image(
        &self,
        model: &str,
        system: &str,
        user: &str,
        image_path: &std::path::Path,
    ) -> Result<String> {
        use base64::{engine::general_purpose::STANDARD, Engine};
        let bytes = std::fs::read(image_path)
            .map_err(|e| anyhow::anyhow!("Failed to read image '{}': {e}", image_path.display()))?;
        let b64 = STANDARD.encode(&bytes);
        self.chat_inner(model, system, user, Some(b64)).await
    }

    async fn chat_inner(
        &self,
        model: &str,
        system: &str,
        user: &str,
        image_b64: Option<String>,
    ) -> Result<String> {
        // Build user message — optionally with images array for vision
        let user_msg = if let Some(b64) = image_b64 {
            serde_json::json!({
                "role": "user",
                "content": user,
                "images": [b64]
            })
        } else {
            serde_json::json!({ "role": "user", "content": user })
        };

        let body = serde_json::json!({
            "model": model,
            "stream": false,
            "options": {
                "temperature": self.temperature,
                "num_predict": self.max_tokens,
            },
            "messages": [
                { "role": "system", "content": system },
                user_msg,
            ]
        });

        let url = format!("{}/api/chat", self.base_url);
        let resp = self
            .http
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("Ollama request failed: {e}\nIs Ollama running at {}?", self.base_url))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            bail!("Ollama returned HTTP {status}: {text}");
        }

        let json: Value = resp.json().await?;
        json.get("message")
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| anyhow::anyhow!("Unexpected Ollama response shape: {json}"))
    }

    /// Check which models are available. Warns for each configured model not found.
    pub async fn check_models(&self, models: &[&str]) -> Result<()> {
        let url = format!("{}/api/tags", self.base_url);
        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("Cannot reach Ollama at {}: {e}", self.base_url))?;

        let json: Value = resp.json().await?;
        let available: Vec<String> = json
            .get("models")
            .and_then(|m| m.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|m| m.get("name").and_then(|n| n.as_str()).map(|s| s.to_string()))
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
