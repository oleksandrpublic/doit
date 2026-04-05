use crate::config_struct::AgentConfig;
use crate::shell::{BackendKind, LlmClient};

impl AgentConfig {
    /// Validate the configuration.
    pub fn validate(&self) -> anyhow::Result<()> {
        // URL
        let url = &self.llm_url;
        if url.trim().is_empty() {
            return Err(anyhow::anyhow!("llm_url cannot be empty"));
        }
        if !url.starts_with("http://") && !url.starts_with("https://") {
            return Err(anyhow::anyhow!(
                "LLM URL must start with http:// or https://: '{url}'"
            ));
        }
        // OpenAI / Anthropic require an API key (unless a local proxy is used — warn only)
        if matches!(
            self.llm_backend,
            BackendKind::OpenAI | BackendKind::Anthropic
        ) && self.effective_api_key().is_none()
        {
            tracing::warn!(
                "llm_backend is '{}' but no api key found. \
                 Set llm_api_key in config.toml or the LLM_API_KEY env var.",
                match self.llm_backend {
                    BackendKind::OpenAI => "openai",
                    _ => "anthropic",
                }
            );
        }
        if self.model.trim().is_empty() {
            return Err(anyhow::anyhow!("model cannot be empty"));
        }
        if self.temperature < 0.0 || self.temperature > 2.0 {
            return Err(anyhow::anyhow!("temperature must be between 0.0 and 2.0"));
        }
        if self.max_tokens == 0 {
            return Err(anyhow::anyhow!("max_tokens must be greater than 0"));
        }
        if self.history_window == 0 {
            return Err(anyhow::anyhow!("history_window must be greater than 0"));
        }
        if self.max_output_chars == 0 {
            return Err(anyhow::anyhow!("max_output_chars must be greater than 0"));
        }
        if self.max_depth == 0 {
            return Err(anyhow::anyhow!("max_depth must be greater than 0"));
        }
        if self.system_prompt.trim().is_empty() {
            return Err(anyhow::anyhow!("system_prompt cannot be empty"));
        }
        if !["error", "warn", "info", "debug", "trace"].contains(&self.log_level.as_str()) {
            return Err(anyhow::anyhow!(
                "log_level must be one of: error, warn, info, debug, trace"
            ));
        }
        if !["text", "json"].contains(&self.log_format.as_str()) {
            return Err(anyhow::anyhow!("log_format must be one of: text, json"));
        }
        for (i, cmd) in self.command_allowlist.iter().enumerate() {
            if cmd.trim().is_empty() {
                return Err(anyhow::anyhow!("command_allowlist[{}] cannot be empty", i));
            }
        }
        let valid_groups = ["browser", "background", "github"];
        for group in &self.tool_groups {
            if !valid_groups.contains(&group.as_str()) {
                return Err(anyhow::anyhow!(
                    "unknown tool_group '{}'. Valid values: {}",
                    group,
                    valid_groups.join(", ")
                ));
            }
        }
        Ok(())
    }

    /// Validate runtime dependencies: LLM connectivity and model availability.
    pub async fn validate_runtime(&self) -> anyhow::Result<()> {
        let client = LlmClient::new(self.backend_config());

        let mut models_to_check = vec![self.model.as_str()];
        for m in [
            self.models.thinking.as_deref(),
            self.models.coding.as_deref(),
            self.models.search.as_deref(),
            self.models.execution.as_deref(),
            self.models.vision.as_deref(),
        ]
        .into_iter()
        .flatten()
        {
            models_to_check.push(m);
        }
        models_to_check.sort();
        models_to_check.dedup();

        client.check_models(&models_to_check).await?;

        if let (Some(token), Some(chat_id)) = (&self.telegram_token, &self.telegram_chat_id) {
            self.validate_telegram(token, chat_id).await?;
        }

        Ok(())
    }

    /// Validate Telegram configuration by testing the API.
    async fn validate_telegram(&self, token: &str, chat_id: &str) -> anyhow::Result<()> {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .map_err(|e| {
                anyhow::anyhow!("Failed to create HTTP client for Telegram validation: {e}")
            })?;

        // First, check if the bot token is valid by calling getMe
        let get_me_url = format!("https://api.telegram.org/bot{}/getMe", token);
        let response = client
            .get(&get_me_url)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("Telegram API request failed: {e}"))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(anyhow::anyhow!(
                "Telegram bot token invalid: HTTP {} - {}",
                status,
                body
            ));
        }

        let json: serde_json::Value = response
            .json()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to parse Telegram getMe response: {e}"))?;

        if !json.get("ok").and_then(|v| v.as_bool()).unwrap_or(false) {
            return Err(anyhow::anyhow!("Telegram getMe returned error: {}", json));
        }

        // Then, check if we can send a message to the chat (but don't actually send one)
        // We can use sendMessage with a test message, but to avoid spamming, maybe just check chat existence
        // Actually, sending a test message is better to ensure full functionality
        let send_message_url = format!("https://api.telegram.org/bot{}/sendMessage", token);
        let payload = serde_json::json!({
            "chat_id": chat_id,
            "text": "do_it configuration test - this message will be deleted automatically",
            "disable_notification": true
        });

        let response = client
            .post(&send_message_url)
            .json(&payload)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("Telegram sendMessage request failed: {e}"))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(anyhow::anyhow!(
                "Cannot send messages to Telegram chat {}: HTTP {} - {}",
                chat_id,
                status,
                body
            ));
        }

        let json: serde_json::Value = response
            .json()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to parse Telegram sendMessage response: {e}"))?;

        if !json.get("ok").and_then(|v| v.as_bool()).unwrap_or(false) {
            return Err(anyhow::anyhow!(
                "Telegram sendMessage returned error: {}",
                json
            ));
        }

        // If we get here, try to delete the test message to clean up
        if let Some(message_id) = json
            .get("result")
            .and_then(|r| r.get("message_id"))
            .and_then(|id| id.as_i64())
        {
            let delete_url = format!("https://api.telegram.org/bot{}/deleteMessage", token);
            let delete_payload = serde_json::json!({
                "chat_id": chat_id,
                "message_id": message_id
            });
            // Don't fail if delete fails, as it's not critical
            let _ = client.post(&delete_url).json(&delete_payload).send().await;
        }

        tracing::info!("Telegram configurations validated successfully");
        Ok(())
    }
}
