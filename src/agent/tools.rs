use serde_json::Value;
use std::path::Path;

use crate::tools::LlmAction;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseActionErrorKind {
    EmptyResponse,
    MissingJson,
    UnterminatedJson,
    InvalidJson,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseActionError {
    kind: ParseActionErrorKind,
    detail: String,
}

impl ParseActionError {
    pub(crate) fn new(kind: ParseActionErrorKind, detail: impl Into<String>) -> Self {
        Self {
            kind,
            detail: detail.into(),
        }
    }

    pub fn kind(&self) -> ParseActionErrorKind {
        self.kind
    }

    pub fn detail(&self) -> &str {
        &self.detail
    }
}

/// Parse LLM response into an action. Handles JSON extraction and recovery.
pub fn parse_action(raw: &str) -> Result<LlmAction, ParseActionError> {
    let cleaned = strip_fences(raw.trim());
    if cleaned.is_empty() {
        tracing::warn!("LLM response was empty");
        return Err(ParseActionError::new(
            ParseActionErrorKind::EmptyResponse,
            "LLM response was empty",
        ));
    }

    let start = match cleaned.find('{') {
        Some(s) => s,
        None => {
            tracing::warn!("LLM response has no JSON: {}", &raw[..raw.len().min(100)]);
            return Err(ParseActionError::new(
                ParseActionErrorKind::MissingJson,
                "LLM response did not contain a JSON object",
            ));
        }
    };

    // Find the matching closing brace by tracking brace depth
    // Must ignore braces inside string literals
    let mut depth = 0;
    let mut end = start;
    let mut in_string = false;
    let mut escape = false;
    for (i, ch) in cleaned[start..].char_indices() {
        if escape {
            escape = false;
            continue;
        }
        if ch == '\\' && in_string {
            escape = true;
            continue;
        }
        if ch == '"' {
            in_string = !in_string;
            continue;
        }
        if in_string {
            continue;
        }
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    end = start + i;
                    break;
                }
            }
            _ => {}
        }
    }

    if depth != 0 {
        // Attempt recovery: close unclosed strings and objects
        let mut recovered = String::from(&cleaned[start..]);
        // Close any open string
        if in_string {
            recovered.push('"');
        }
        // Close remaining open objects
        for _ in 0..depth {
            recovered.push('}');
        }
        tracing::warn!(
            "LLM response had unclosed JSON, attempting recovery: {}",
            &raw[..raw.len().min(100)]
        );
        return match serde_json::from_str::<LlmAction>(&recovered) {
            Ok(action) => Ok(action),
            Err(e) => {
                tracing::warn!("LLM JSON recovery failed: {e}");
                Err(ParseActionError::new(
                    ParseActionErrorKind::UnterminatedJson,
                    format!("Unterminated JSON could not be recovered: {e}"),
                ))
            }
        };
    }

    let json_str = &cleaned[start..=end];
    match serde_json::from_str::<LlmAction>(json_str) {
        Ok(action) => Ok(action),
        Err(e) => {
            tracing::warn!("Failed to parse LLM JSON: {e}");
            Err(ParseActionError::new(
                ParseActionErrorKind::InvalidJson,
                format!("Invalid action JSON: {e}"),
            ))
        }
    }
}

/// Strip markdown code fences from LLM response.
pub fn strip_fences(s: &str) -> &str {
    let s = s.strip_prefix("```json").unwrap_or(s);
    let s = s.strip_prefix("```").unwrap_or(s);
    let s = s.strip_suffix("```").unwrap_or(s);
    s.trim()
}

/// Get first line of output, truncated to max length.
pub fn first_line(s: &str, max: usize) -> String {
    let line = s.lines().next().unwrap_or("").trim();
    let mut chars = line.chars();
    let collected: String = chars.by_ref().take(max).collect();
    if chars.next().is_some() {
        format!("{collected}…")
    } else {
        collected
    }
}

/// Format action args for display (truncate long output).
pub fn format_args_display(args: &Value) -> String {
    let json = serde_json::to_string(args).unwrap_or("{}".to_string());
    // Replace escaped \n with actual newlines
    let unescaped = json.replace("\\n", "\n");
    // Take first 3 lines, add "..." if there are more
    let mut lines: Vec<&str> = unescaped.lines().collect();
    if lines.len() > 3 {
        lines.truncate(3);
        lines.push("...");
    }
    lines.join("\n")
}

/// Detect project name from Cargo.toml or package.json.
pub fn detect_project_name(root: &Path) -> Option<String> {
    // Try Cargo.toml first
    if let Ok(s) = std::fs::read_to_string(root.join("Cargo.toml")) {
        for line in s.lines() {
            let line = line.trim();
            if line.starts_with("name") {
                if let Some(val) = line.split_once('=').map(|x| x.1) {
                    return Some(val.trim().trim_matches('"').to_string());
                }
            }
        }
    }
    // Try package.json
    if let Ok(s) = std::fs::read_to_string(root.join("package.json")) {
        if let Ok(json) = serde_json::from_str::<Value>(&s) {
            if let Some(name) = json.get("name").and_then(|v| v.as_str()) {
                return Some(name.to_string());
            }
        }
    }
    None
}

/// Detect GitHub repo from .git/config.
pub fn detect_github_repo(root: &Path) -> Option<String> {
    let config = std::fs::read_to_string(root.join(".git/config")).ok()?;
    for line in config.lines() {
        let line = line.trim();
        if line.starts_with("url =") {
            let url = line.split_once('=')?.1.trim();

            // https://github.com/owner/repo.git  or  git@github.com:owner/repo.git
            let repo = if let Some(rest) = url.strip_prefix("https://github.com/") {
                rest.trim_end_matches(".git")
            } else if let Some(rest) = url.strip_prefix("git@github.com:") {
                rest.trim_end_matches(".git")
            } else {
                continue;
            };
            return Some(repo.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{ParseActionErrorKind, parse_action};

    #[test]
    fn parse_action_reports_empty_response() {
        let err = parse_action("   ").unwrap_err();
        assert_eq!(err.kind(), ParseActionErrorKind::EmptyResponse);
        assert!(err.detail().contains("empty"));
    }

    #[test]
    fn parse_action_reports_missing_json() {
        let err = parse_action("thought: inspect config first").unwrap_err();
        assert_eq!(err.kind(), ParseActionErrorKind::MissingJson);
    }

    #[test]
    fn parse_action_recovers_unterminated_json_when_shape_is_otherwise_valid() {
        let action = parse_action(
            r#"{"thought":"Inspect config","tool":"read_file","args":{"path":"src/config.rs""#,
        )
        .expect("recovery should succeed");

        assert_eq!(action.tool, "read_file");
        assert_eq!(action.thought, "Inspect config");
        assert_eq!(action.args["path"], "src/config.rs");
    }

    #[test]
    fn parse_action_reports_invalid_json_for_wrong_schema() {
        let err = parse_action(r#"{"thought":"Inspect config","tool":12,"args":{}}"#).unwrap_err();
        assert_eq!(err.kind(), ParseActionErrorKind::InvalidJson);
        assert!(err.detail().contains("Invalid action JSON"));
    }
}
