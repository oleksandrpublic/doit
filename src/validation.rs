//! Centralized validation module for argument validation, path resolution, and security checks

use anyhow::Result;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use url::Url;

/// Resolve a path safely, preventing directory traversal attacks
pub fn resolve_safe_path(root: &Path, path: &str) -> Result<PathBuf> {
    if path.starts_with("..") || path.contains("../") || path.contains("..\\") {
        let full_path = root.join(path);
        let canonical_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        return if full_path.exists() {
            let canonical_full = full_path.canonicalize()?;
            if !canonical_full.starts_with(&canonical_root) {
                anyhow::bail!("Path traversal detected: {} escapes repository root", path);
            }
            Ok(canonical_full)
        } else {
            let normalized = normalize_path(&full_path);
            let root_normalized = normalize_path(&canonical_root);
            if !normalized.starts_with(&root_normalized) {
                anyhow::bail!("Path traversal detected: {} escapes repository root", path);
            }
            Ok(full_path)
        };
    }
    let full_path = root.join(path);
    let canonical_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    if !full_path.exists() {
        let normalize = |p: &Path| -> PathBuf {
            let s = p.to_string_lossy();
            if let Some(stripped) = s.strip_prefix("\\\\?\\") {
                PathBuf::from(stripped)
            } else {
                p.to_path_buf()
            }
        };
        let root_norm = normalize(&canonical_root);
        let full_norm = normalize(&full_path);
        if full_norm.starts_with(&root_norm) {
            return Ok(full_path);
        }
        anyhow::bail!("Path traversal detected: {} escapes repository root", path);
    }
    let canonical_full = full_path.canonicalize()?;
    if canonical_full.starts_with(&canonical_root) {
        Ok(canonical_full)
    } else {
        anyhow::bail!("Path traversal detected: {} escapes repository root", path)
    }
}

pub fn resolve_memory_path(root: &Path, path: &str) -> Result<PathBuf> {
    let ai_dir = root.join(".ai");
    resolve_safe_path(&ai_dir, path)
}

pub fn normalize_path(path: &Path) -> PathBuf {
    let mut components = Vec::new();
    for component in path.components() {
        match component {
            std::path::Component::ParentDir => {
                components.pop();
            }
            std::path::Component::CurDir => {}
            _ => components.push(component),
        }
    }
    components.iter().collect()
}

pub fn str_arg(args: &HashMap<String, String>, key: &str) -> Option<String> {
    args.get(key).cloned()
}

pub fn num_arg(args: &HashMap<String, String>, key: &str) -> Option<i64> {
    args.get(key).and_then(|s| s.parse().ok())
}

pub fn bool_arg(args: &HashMap<String, String>, key: &str) -> Option<bool> {
    args.get(key).and_then(|s| match s.as_str() {
        "true" | "yes" | "1" => Some(true),
        "false" | "no" | "0" => Some(false),
        _ => None,
    })
}

pub fn validate_required(args: &HashMap<String, String>, key: &str) -> Result<()> {
    if args.contains_key(key) {
        Ok(())
    } else {
        anyhow::bail!("Missing required argument: {}", key)
    }
}

pub fn validate_file_content(path: &Path) -> Result<()> {
    if !path.exists() {
        anyhow::bail!("File does not exist: {}", path.display());
    }
    if !path.is_file() {
        anyhow::bail!("Not a file: {}", path.display());
    }
    Ok(())
}

pub fn validate_program(program: &str) -> Result<()> {
    use std::env::consts::EXE_SUFFIX;
    let program_with_ext = format!("{}{}", program, EXE_SUFFIX);
    if let Ok(paths) = std::env::var("PATH") {
        for path in std::env::split_paths(&paths) {
            if path.join(&program_with_ext).exists() || path.join(program).exists() {
                return Ok(());
            }
        }
    }
    anyhow::bail!("Program not found in PATH: {}", program)
}

pub fn validate_commit_message(msg: &str) -> Result<()> {
    if msg.is_empty() {
        anyhow::bail!("Commit message cannot be empty");
    }
    if msg.len() > 500 {
        anyhow::bail!("Commit message too long (max 500 chars)");
    }
    Ok(())
}

/// Validate GitHub token format
/// GitHub tokens have specific prefixes:
/// - ghp_ : Personal Access Token (classic)
/// - github_pat_ : Fine-grained Personal Access Token
/// - gho_ : OAuth authorization
/// - ghs_ : GitHub App installation access token
/// - ghr_ : Refresh token
pub fn validate_github_token(token: &str) -> Result<()> {
    if token.is_empty() {
        return Ok(()); // Empty token is allowed for unauthenticated requests
    }
    let valid_prefixes = ["ghp_", "github_pat_", "gho_", "ghs_", "ghr_"];
    if !valid_prefixes
        .iter()
        .any(|prefix| token.starts_with(prefix))
    {
        anyhow::bail!("Invalid GitHub token format. Token should start with ghp_, github_pat_, gho_, ghs_, or ghr_");
    }
    // Basic length check - tokens should be at least 40 characters after prefix
    let min_len = 40;
    if token.len() < min_len {
        anyhow::bail!("GitHub token too short (minimum {} characters)", min_len);
    }
    Ok(())
}

pub fn validate_url_safe(url: &str) -> Result<()> {
    let parsed = Url::parse(url).map_err(|e| anyhow::anyhow!("Invalid URL: {}", e))?;
    let scheme = parsed.scheme().to_lowercase();
    if scheme != "http" && scheme != "https" {
        anyhow::bail!("Only http and https schemes are allowed");
    }
    let host = parsed.host_str().unwrap_or("");
    if host.eq_ignore_ascii_case("localhost")
        || host.eq_ignore_ascii_case("127.0.0.1")
        || host.eq_ignore_ascii_case("::1")
    {
        anyhow::bail!("SSRF protection: localhost/loopback addresses are blocked");
    }
    if host.eq_ignore_ascii_case("link-local")
        || host.starts_with("fe80:")
        || host.starts_with("ff")
    {
        anyhow::bail!("SSRF protection: link-local/multicast addresses are blocked");
    }
    if let Some(ip) = parsed.host() {
        match ip {
            url::Host::Ipv4(ipv4) => {
                let octets = ipv4.octets();
                if octets[0] == 10 {
                    anyhow::bail!("SSRF protection: private IP range 10.x.x.x is blocked");
                }
                if octets[0] == 172 && octets[1] >= 16 && octets[1] <= 31 {
                    anyhow::bail!("SSRF protection: private IP range 172.16-31.x.x is blocked");
                }
                if octets[0] == 192 && octets[1] == 168 {
                    anyhow::bail!("SSRF protection: private IP range 192.168.x.x is blocked");
                }
                if octets[0] == 169 && octets[1] == 254 {
                    anyhow::bail!("SSRF protection: link-local IP range 169.254.x.x is blocked");
                }
                if octets[0] == 0 && octets[1] == 0 && octets[2] == 0 && octets[3] == 0 {
                    anyhow::bail!("SSRF protection: 0.0.0.0 is blocked");
                }
            }
            url::Host::Ipv6(ipv6) => {
                let segments = ipv6.segments();
                if segments[0] == 0
                    && segments[1] == 0
                    && segments[2] == 0
                    && segments[3] == 0
                    && segments[4] == 0
                    && segments[5] == 0
                    && segments[6] == 0
                    && segments[7] == 1
                {
                    anyhow::bail!("SSRF protection: IPv6 loopback ::1 is blocked");
                }
                if segments[0] & 0xfe00 == 0xfc00 {
                    anyhow::bail!("SSRF protection: IPv6 unique local range is blocked");
                }
                if segments[0] & 0xffc0 == 0xfe80 {
                    anyhow::bail!("SSRF protection: IPv6 link-local range is blocked");
                }
            }
            url::Host::Domain(_) => {}
        }
    }
    Ok(())
}

pub fn validate_url_allowlist(url: &str, allowlist: &[String]) -> Result<()> {
    if allowlist.is_empty() {
        return Ok(());
    }
    let parsed = Url::parse(url).map_err(|e| anyhow::anyhow!("Invalid URL: {}", e))?;
    let host = parsed.host_str().unwrap_or("").to_lowercase();
    for allowed in allowlist {
        let allowed_lower = allowed.to_lowercase();
        if host == allowed_lower || host.ends_with(&format!(".{}", allowed_lower)) {
            return Ok(());
        }
    }
    anyhow::bail!("URL host '{}' is not in the allowlist", host)
}

pub fn validate_url_blocklist(url: &str, blocklist: &[String]) -> Result<()> {
    if blocklist.is_empty() {
        return Ok(());
    }
    let parsed = Url::parse(url).map_err(|e| anyhow::anyhow!("Invalid URL: {}", e))?;
    let host = parsed.host_str().unwrap_or("").to_lowercase();
    for blocked in blocklist {
        let blocked_lower = blocked.to_lowercase();
        if host == blocked_lower || host.ends_with(&format!(".{}", blocked_lower)) {
            anyhow::bail!("URL host '{}' is in the blocklist", host)
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_resolve_safe_path_normal() {
        let temp_dir = TempDir::new().unwrap();
        let child = temp_dir.path().join("subdir");
        fs::create_dir(&child).unwrap();
        let file_path = child.join("file.txt");
        fs::write(&file_path, "content").unwrap();
        let result = resolve_safe_path(temp_dir.path(), "subdir/file.txt");
        assert!(result.is_ok());
    }

    #[test]
    fn test_resolve_safe_path_traversal() {
        let temp_dir = TempDir::new().unwrap();
        let result = resolve_safe_path(temp_dir.path(), "../escape.txt");
        assert!(result.is_err());
    }

    #[test]
    fn test_str_arg() {
        let mut args = HashMap::new();
        args.insert("name".to_string(), "test".to_string());
        assert_eq!(str_arg(&args, "name"), Some("test".to_string()));
        assert_eq!(str_arg(&args, "missing"), None);
    }

    #[test]
    fn test_num_arg() {
        let mut args = HashMap::new();
        args.insert("count".to_string(), "42".to_string());
        assert_eq!(num_arg(&args, "count"), Some(42));
        args.insert("bad".to_string(), "not_a_number".to_string());
        assert_eq!(num_arg(&args, "bad"), None);
    }

    #[test]
    fn test_bool_arg() {
        let mut args = HashMap::new();
        args.insert("flag".to_string(), "true".to_string());
        assert_eq!(bool_arg(&args, "flag"), Some(true));
        args.insert("flag".to_string(), "false".to_string());
        assert_eq!(bool_arg(&args, "flag"), Some(false));
        args.insert("flag".to_string(), "maybe".to_string());
        assert_eq!(bool_arg(&args, "flag"), None);
    }

    #[test]
    fn test_validate_required() {
        let mut args = HashMap::new();
        args.insert("path".to_string(), "/test".to_string());
        assert!(validate_required(&args, "path").is_ok());
        assert!(validate_required(&args, "missing").is_err());
    }

    #[test]
    fn test_validate_commit_message() {
        assert!(validate_commit_message("feat: add feature").is_ok());
        assert!(validate_commit_message("").is_err());
        assert!(validate_commit_message(&"a".repeat(501)).is_err());
    }

    #[test]
    fn test_validate_url_safe() {
        assert!(validate_url_safe("https://example.com").is_ok());
        assert!(validate_url_safe("http://localhost").is_err());
        assert!(validate_url_safe("http://127.0.0.1").is_err());
        assert!(validate_url_safe("http://192.168.1.1").is_err());
    }
}
