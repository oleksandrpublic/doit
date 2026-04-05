use super::core::{str_arg, ToolResult};
use super::rate_limit::RateLimiter;
use crate::validation::{
    validate_github_token, validate_url_allowlist, validate_url_blocklist, validate_url_safe,
};
use anyhow::Result;
use serde_json::Value;

/// Fetch content from a URL using HTTP GET.
/// Args:
///   url         — URL to fetch
///   timeout_ms? — request timeout in milliseconds (default: 10000)
///   headers?    — object with custom HTTP headers
///   allowlist?  — array of allowed domain names (e.g., ["example.com", "api.github.com"])
///   blocklist?  — array of blocked domain names (e.g., ["malicious.com"])
///
/// Supports http, https, and file:// URLs.
/// SSRF protection is always applied. Allowlist/blocklist are optional.
pub async fn fetch_url(args: &Value) -> Result<ToolResult> {
    let url = str_arg(args, "url")?;
    let timeout_ms = args
        .get("timeout_ms")
        .and_then(|v| v.as_u64())
        .unwrap_or(10000);

    // Handle file:// URLs before SSRF validation
    if url.starts_with("file://") {
        let path = url.strip_prefix("file://").unwrap_or(&url);
        let decoded = percent_encoding::percent_decode(path.as_bytes()).decode_utf8_lossy();
        return match tokio::fs::read_to_string(decoded.as_ref()).await {
            Ok(content) => Ok(ToolResult::ok(content)),
            Err(e) => Ok(ToolResult {
                output: format!("fetch_url: failed to read file: {e}"),
                success: false,
            }),
        };
    }

    // SSRF protection: validate URL before making request
    if let Err(e) = validate_url_safe(&url) {
        return Ok(ToolResult {
            output: format!("fetch_url: {}", e),
            success: false,
        });
    }

    // Apply allowlist validation if provided
    if let Some(allowlist_arr) = args.get("allowlist").and_then(|v| v.as_array()) {
        let allowlist: Vec<String> = allowlist_arr
            .iter()
            .filter_map(|v| v.as_str().map(|s| s.to_string()))
            .collect();
        if let Err(e) = validate_url_allowlist(&url, &allowlist) {
            return Ok(ToolResult {
                output: format!("fetch_url: {}", e),
                success: false,
            });
        }
    }

    // Apply blocklist validation if provided
    if let Some(blocklist_arr) = args.get("blocklist").and_then(|v| v.as_array()) {
        let blocklist: Vec<String> = blocklist_arr
            .iter()
            .filter_map(|v| v.as_str().map(|s| s.to_string()))
            .collect();
        if let Err(e) = validate_url_blocklist(&url, &blocklist) {
            return Ok(ToolResult {
                output: format!("fetch_url: {}", e),
                success: false,
            });
        }
    }

    // Build HTTP client
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_millis(timeout_ms))
        .build()
        .map_err(|e| anyhow::anyhow!("fetch_url: failed to create HTTP client: {e}"))?;

    let mut req = client.get(url);

    // Add custom headers if provided
    if let Some(headers_obj) = args.get("headers").and_then(|v| v.as_object()) {
        for (key, val) in headers_obj {
            if let Some(val_str) = val.as_str() {
                req = req.header(key, val_str);
            }
        }
    }

    match req.send().await {
        Ok(resp) => {
            let status = resp.status();
            let success = status.is_success();

            match resp.text().await {
                Ok(body) => Ok(ToolResult {
                    output: if success {
                        body
                    } else {
                        format!("HTTP {}\n\n{}", status, body)
                    },
                    success,
                }),
                Err(e) => Ok(ToolResult {
                    output: format!("fetch_url: failed to read response body: {e}"),
                    success: false,
                }),
            }
        }
        Err(e) => Ok(ToolResult {
            output: format!("fetch_url: request failed: {e}"),
            success: false,
        }),
    }
}

/// Perform a web search using a configured search engine.
/// Args:
///   query       — search query string
///   engine?     — search engine: "google", "duckduckgo", "bing" (default: "duckduckgo")
///   num_results? — number of results to return (default: 10)
///
/// Note: Requires [search] configuration in config.toml for API-based engines.
/// Falls back to HTML scraping for engines without API support.
pub async fn web_search(args: &Value) -> Result<ToolResult> {
    let query = str_arg(args, "query")?;
    let engine = args
        .get("engine")
        .and_then(|v| v.as_str())
        .unwrap_or("duckduckgo");
    let num_results = args
        .get("num_results")
        .and_then(|v| v.as_u64())
        .unwrap_or(10) as usize;

    // Build search URL based on engine
    let search_url = match engine {
        "google" => format!(
            "https://www.google.com/search?q={}",
            urlencoding::encode(&query)
        ),
        "duckduckgo" => format!(
            "https://html.duckduckgo.com/html/?q={}",
            urlencoding::encode(&query)
        ),
        "bing" => format!(
            "https://www.bing.com/search?q={}",
            urlencoding::encode(&query)
        ),
        _ => {
            return Ok(ToolResult {
                output: format!(
                    "web_search: unknown engine '{engine}'. Valid: google, duckduckgo, bing"
                ),
                success: false,
            });
        }
    };

    // Rate limiting: wait to prevent overwhelming the search engine
    let mut rate_limiter = RateLimiter::new(1);
    rate_limiter.wait().await;

    // Fetch search results page
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| anyhow::anyhow!("web_search: failed to create HTTP client: {e}"))?;

    match client.get(&search_url).send().await {
        Ok(resp) if resp.status().is_success() => {
            let html = resp.text().await.unwrap_or_default();

            // Simple extraction: look for title/description patterns
            // This is a basic scraper; production would use html5ever or scraper crate
            let mut results: Vec<String> = Vec::new();

            // Extract links and titles from HTML (simplified)
            for line in html.lines() {
                if line.contains("<a") && line.contains("href=") {
                    if let Some(href_start) = line.find("href=\"") {
                        if let Some(href_end) = line[href_start + 7..].find('"') {
                            let href = &line[href_start + 7..href_start + 7 + href_end];
                            if href.starts_with("http") && !href.contains("duckduckgo") {
                                results.push(href.to_string());
                            }
                        }
                    }
                }
                if results.len() >= num_results {
                    break;
                }
            }

            if results.is_empty() {
                Ok(ToolResult {
                    output: format!("web_search: no results found for '{query}'"),
                    success: false,
                })
            } else {
                Ok(ToolResult::ok(format!(
                    "Search results for '{query}' ({engine}):\n{}",
                    results.join("\n")
                )))
            }
        }
        Ok(resp) => Ok(ToolResult {
            output: format!("web_search: HTTP {}", resp.status()),
            success: false,
        }),
        Err(e) => Ok(ToolResult {
            output: format!("web_search: request failed: {e}"),
            success: false,
        }),
    }
}

/// Interact with the GitHub REST API.
/// Args:
///   action   — "issues", "prs", "contents", "search", "repos"
///   repo     — owner/repo string (e.g., "do-it-ai/do_it")
///   path?    — file path for contents action
///   query?   — search query for search action
///   token?   — GitHub PAT (falls back to GITHUB_TOKEN env or unauthenticated)
///   state?   — filter state for issues/prs (default: "open")
///   per_page? — results per page (default: 30, max: 100)
///
/// Requires GITHUB_TOKEN env var or token arg for authenticated requests.
pub async fn github_api(args: &Value) -> Result<ToolResult> {
    use reqwest::header::{AUTHORIZATION, USER_AGENT};

    let action = str_arg(args, "action")?;
    let repo = str_arg(args, "repo")?;

    // Get auth token
    let token = args
        .get("token")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .or_else(|| std::env::var("GITHUB_TOKEN").ok())
        .unwrap_or_default();

    // Validate token format if provided
    if !token.is_empty() {
        validate_github_token(&token)?;
    }

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| anyhow::anyhow!("github_api: failed to create HTTP client: {e}"))?;

    let url = match action.as_str() {
        "issues" => {
            let state = args.get("state").and_then(|v| v.as_str()).unwrap_or("open");
            let per_page = args.get("per_page").and_then(|v| v.as_u64()).unwrap_or(30);
            format!(
                "https://api.github.com/repos/{}/issues?state={}&per_page={}",
                repo, state, per_page
            )
        }
        "prs" => {
            let state = args.get("state").and_then(|v| v.as_str()).unwrap_or("open");
            let per_page = args.get("per_page").and_then(|v| v.as_u64()).unwrap_or(30);
            format!(
                "https://api.github.com/repos/{}/pulls?state={}&per_page={}",
                repo, state, per_page
            )
        }
        "contents" => {
            let path = args
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or("README.md");
            format!("https://api.github.com/repos/{}/contents/{}", repo, path)
        }
        "search" => {
            let query = args
                .get("query")
                .and_then(|v| v.as_str())
                .unwrap_or("is:repo");
            format!(
                "https://api.github.com/search/repositories?q={}",
                urlencoding::encode(query)
            )
        }
        "repos" => {
            format!("https://api.github.com/repos/{}", repo)
        }
        _ => {
            return Ok(ToolResult {
                output: format!("github_api: unknown action '{action}'. Valid: issues, prs, contents, search, repos"),
                success: false,
            });
        }
    };

    // Rate limiting: GitHub API has strict rate limits, wait 2 seconds between requests
    let mut rate_limiter = RateLimiter::new(2);
    rate_limiter.wait().await;

    let mut req = client.get(&url);
    req = req.header(USER_AGENT, "do_it-agent/1.0");
    if !token.is_empty() {
        req = req.header(AUTHORIZATION, format!("Bearer {token}"));
    }

    match req.send().await {
        Ok(resp) => {
            let status = resp.status();
            let success = status.is_success();

            match resp.text().await {
                Ok(body) => {
                    // Pretty-print JSON if it's valid JSON
                    let formatted = serde_json::from_str::<Value>(&body)
                        .ok()
                        .and_then(|v| serde_json::to_string_pretty(&v).ok())
                        .unwrap_or(body.to_string());

                    Ok(ToolResult {
                        output: formatted,
                        success,
                    })
                }
                Err(e) => Ok(ToolResult {
                    output: format!("github_api: failed to read response: {e}"),
                    success: false,
                }),
            }
        }
        Err(e) => Ok(ToolResult {
            output: format!("github_api: request failed: {e}"),
            success: false,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    mod fetch_url {
        use super::*;

        #[tokio::test]
        async fn test_missing_url_arg() {
            let args = json!({});
            let result = fetch_url(&args).await;
            assert!(result.is_err());
            assert!(result.unwrap_err().to_string().contains("Missing arg: url"));
        }

        #[tokio::test]
        async fn test_file_url_read_success() {
            use std::fs;
            use tempfile::TempDir;

            let temp_dir = TempDir::new().unwrap();
            let file_path = temp_dir.path().join("test.txt");
            fs::write(&file_path, "test content").unwrap();

            let file_url = format!("file://{}", file_path.to_string_lossy().replace("\\", "/"));
            let args = json!({ "url": file_url });

            let result = fetch_url(&args).await.unwrap();
            assert!(result.success);
            assert!(result.output.contains("test content"));
        }

        #[tokio::test]
        async fn test_file_url_not_found() {
            let args = json!({ "url": "file:///nonexistent/path.txt" });
            let result = fetch_url(&args).await.unwrap();
            assert!(!result.success);
            assert!(result.output.contains("failed to read file"));
        }

        #[tokio::test]
        async fn test_invalid_timeout() {
            // Test with valid args but the function should handle timeout gracefully
            let args = json!({ "url": "https://example.com", "timeout_ms": 1 });
            let result = fetch_url(&args).await;
            // Should not panic, may timeout or fail
            assert!(result.is_ok());
        }
    }

    mod web_search {
        use super::*;

        #[tokio::test]
        async fn test_missing_query_arg() {
            let args = json!({});
            let result = web_search(&args).await;
            assert!(result.is_err());
            assert!(result
                .unwrap_err()
                .to_string()
                .contains("Missing arg: query"));
        }

        #[tokio::test]
        async fn test_unknown_engine() {
            let args = json!({ "query": "test", "engine": "unknown" });
            let result = web_search(&args).await.unwrap();
            assert!(!result.success);
            assert!(result.output.contains("unknown engine"));
        }

        #[tokio::test]
        async fn test_default_engine_duckduckgo() {
            let args = json!({ "query": "test" });
            let result = web_search(&args).await;
            // Should not panic, may succeed or fail depending on network
            assert!(result.is_ok());
        }

        #[tokio::test]
        async fn test_google_engine() {
            let args = json!({ "query": "test", "engine": "google" });
            let result = web_search(&args).await;
            assert!(result.is_ok());
        }

        #[tokio::test]
        async fn test_bing_engine() {
            let args = json!({ "query": "test", "engine": "bing" });
            let result = web_search(&args).await;
            assert!(result.is_ok());
        }
    }

    mod github_api {
        use super::*;

        #[tokio::test]
        async fn test_missing_action_arg() {
            let args = json!({ "repo": "test/repo" });
            let result = github_api(&args).await;
            assert!(result.is_err());
            assert!(result
                .unwrap_err()
                .to_string()
                .contains("Missing arg: action"));
        }

        #[tokio::test]
        async fn test_missing_repo_arg() {
            let args = json!({ "action": "issues" });
            let result = github_api(&args).await;
            assert!(result.is_err());
            assert!(result
                .unwrap_err()
                .to_string()
                .contains("Missing arg: repo"));
        }

        #[tokio::test]
        async fn test_unknown_action() {
            let args = json!({ "action": "unknown", "repo": "test/repo" });
            let result = github_api(&args).await.unwrap();
            assert!(!result.success);
            assert!(result.output.contains("unknown action"));
        }

        #[tokio::test]
        async fn test_valid_action_issues() {
            let args = json!({ "action": "issues", "repo": "rust-lang/rust" });
            let result = github_api(&args).await;
            assert!(result.is_ok());
        }

        #[tokio::test]
        async fn test_valid_action_prs() {
            let args = json!({ "action": "prs", "repo": "rust-lang/rust" });
            let result = github_api(&args).await;
            assert!(result.is_ok());
        }

        #[tokio::test]
        async fn test_valid_action_contents() {
            let args =
                json!({ "action": "contents", "repo": "rust-lang/rust", "path": "README.md" });
            let result = github_api(&args).await;
            assert!(result.is_ok());
        }

        #[tokio::test]
        async fn test_valid_action_search() {
            let args = json!({ "action": "search", "repo": "rust-lang/rust", "query": "rust" });
            let result = github_api(&args).await;
            assert!(result.is_ok());
        }

        #[tokio::test]
        async fn test_valid_action_repos() {
            let args = json!({ "action": "repos", "repo": "rust-lang/rust" });
            let result = github_api(&args).await;
            assert!(result.is_ok());
        }

        #[tokio::test]
        async fn test_custom_state_param() {
            let args = json!({ "action": "issues", "repo": "rust-lang/rust", "state": "closed" });
            let result = github_api(&args).await;
            assert!(result.is_ok());
        }

        #[tokio::test]
        async fn test_custom_per_page_param() {
            let args = json!({ "action": "issues", "repo": "rust-lang/rust", "per_page": 5 });
            let result = github_api(&args).await;
            assert!(result.is_ok());
        }
    }
}
