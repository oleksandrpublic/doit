//! Central redaction policy for session and tool artifacts.
//!
//! Provides a shared helper for scrubbing sensitive snippets from
//! text before it is written to logs, traces, or user-visible output.

const REDACTED: &str = "[redacted]";

/// Tokens that should never appear verbatim in session artifacts.
///
/// Extend this list when new sensitive token patterns are identified.
/// Matching is case-insensitive and applies to the raw text before it
/// reaches any log or trace sink.
static SENSITIVE_TOKENS: &[&str] = &[
    "-----BEGIN ",               // PEM private keys, certificates
    "sk-",                       // OpenAI-style API keys
    "ghp_",                      // GitHub personal access tokens
    "ghs_",                      // GitHub Actions tokens
    "glpat-",                    // GitLab personal access tokens
    "xoxb-",                     // Slack bot tokens
    "xoxp-",                     // Slack user tokens
    "Authorization: Bearer ",
    "Authorization: Basic ",
    "password=",
    "passwd=",
    "secret=",
    "api_key=",
    "apikey=",
    "access_token=",
    "refresh_token=",
    "private_key=",
];

/// Redact known-sensitive token patterns from `text`.
///
/// Each line is checked independently.  If a line contains a sensitive
/// token the entire line is replaced with `[redacted]`.  All other lines
/// are passed through unchanged.
///
/// This is intentionally conservative: it replaces full lines rather than
/// attempting to extract just the secret value, so callers never leak a
/// partial token.
pub fn redact(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for (i, line) in text.lines().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        if line_is_sensitive(line) {
            out.push_str(REDACTED);
        } else {
            out.push_str(line);
        }
    }
    // Preserve a trailing newline when the original had one.
    if text.ends_with('\n') && !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

fn line_is_sensitive(line: &str) -> bool {
    let lower = line.to_lowercase();
    SENSITIVE_TOKENS
        .iter()
        .any(|token| lower.contains(&token.to_lowercase()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redact_passes_through_clean_text() {
        let input = "hello world\nno secrets here\n";
        assert_eq!(redact(input), input);
    }

    #[test]
    fn redact_replaces_pem_line() {
        let input = "config = ok\n-----BEGIN RSA PRIVATE KEY-----\nmore text\n";
        let result = redact(input);
        assert!(result.contains("[redacted]"));
        assert!(!result.contains("BEGIN RSA"));
        assert!(result.contains("config = ok"));
        assert!(result.contains("more text"));
    }

    #[test]
    fn redact_replaces_api_key_assignment_line() {
        let input = "host=localhost\napi_key=sk-supersecret\nport=8080\n";
        let result = redact(input);
        assert!(result.contains("[redacted]"));
        assert!(!result.contains("supersecret"));
        assert!(result.contains("host=localhost"));
        assert!(result.contains("port=8080"));
    }

    #[test]
    fn redact_replaces_bearer_token_line() {
        let input = "Authorization: Bearer eyJhbGciOiJIUzI1NiJ9.payload.sig";
        let result = redact(input);
        assert_eq!(result, "[redacted]");
    }

    #[test]
    fn redact_is_case_insensitive() {
        let input = "API_KEY=topsecret123\n";
        let result = redact(input);
        assert!(result.contains("[redacted]"));
        assert!(!result.contains("topsecret"));
    }

    #[test]
    fn redact_preserves_trailing_newline() {
        let input = "clean line\n";
        assert!(redact(input).ends_with('\n'));
    }

    #[test]
    fn redact_handles_empty_string() {
        assert_eq!(redact(""), "");
    }

    #[test]
    fn redact_handles_multiline_with_multiple_sensitive_lines() {
        let input = "ok\npassword=hunter2\napi_key=abc123\nok2\n";
        let result = redact(input);
        assert_eq!(result, "ok\n[redacted]\n[redacted]\nok2\n");
    }
}
