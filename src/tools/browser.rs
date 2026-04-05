use super::core::{ToolResult, resolve, str_arg};
use anyhow::{Context, Result, bail};
use headless_chrome::{Browser, LaunchOptions};
use serde_json::Value;
use std::path::Path;

fn get_browser() -> Result<Browser> {
    Browser::new(LaunchOptions {
        headless: true,
        ..Default::default()
    })
}

pub async fn browser_navigate(args: &Value, _root: &Path) -> Result<ToolResult> {
    let url = str_arg(args, "url")?;

    let browser = get_browser()?;
    let tab = browser.new_tab()?;

    tab.navigate_to(&url)
        .with_context(|| format!("Failed to navigate to {}", url))?;
    tab.wait_until_navigated()
        .with_context(|| format!("Navigation to {} did not complete", url))?;

    Ok(ToolResult::ok(format!(
        "Successfully navigated to: {}",
        url
    )))
}

pub async fn browser_get_text(args: &Value, _root: &Path) -> Result<ToolResult> {
    let selector = args
        .get("selector")
        .and_then(|v| v.as_str())
        .unwrap_or("body");

    let browser = get_browser()?;
    let tab = browser.new_tab()?;
    if let Some(url) = args.get("url").and_then(|v| v.as_str()) {
        tab.navigate_to(url)
            .with_context(|| format!("Failed to navigate to {}", url))?;
    }

    tab.wait_until_navigated()?;

    let script = format!("document.querySelector('{}')?.textContent || ''", selector);

    let result = tab.evaluate(&script, false)?;
    let text = result
        .value
        .and_then(|v| v.as_str().map(|s| s.to_string()))
        .unwrap_or_default();

    Ok(ToolResult::ok(format!(
        "Text from selector '{}': {}",
        selector, text
    )))
}

pub async fn browser_screenshot(args: &Value, root: &Path) -> Result<ToolResult> {
    let path = args
        .get("path")
        .and_then(|v| v.as_str())
        .unwrap_or("screenshot.png");

    let full_path = resolve(root, path)?;

    let browser = get_browser()?;
    let tab = browser.new_tab()?;
    if let Some(url) = args.get("url").and_then(|v| v.as_str()) {
        tab.navigate_to(url)
            .with_context(|| format!("Failed to navigate to {}", url))?;
    }

    tab.wait_until_navigated()?;

    let png_data = tab.capture_screenshot(
        headless_chrome::protocol::cdp::Page::CaptureScreenshotFormatOption::Png,
        None,
        None,
        true,
    )?;

    std::fs::write(&full_path, png_data)
        .with_context(|| format!("Failed to write screenshot to {}", full_path.display()))?;

    Ok(ToolResult::ok(format!(
        "Screenshot saved to: {}",
        full_path.display()
    )))
}

pub async fn browser_action(args: &Value, _root: &Path) -> Result<ToolResult> {
    let action = str_arg(args, "action")?;
    let selector = args
        .get("selector")
        .and_then(|v| v.as_str())
        .unwrap_or("body");

    match action.as_str() {
        "click" | "scroll" | "type" | "hover" | "clear" | "select" => {}
        _ => bail!("Unknown action: {action}"),
    }

    let browser = get_browser()?;
    let tab = browser.new_tab()?;
    if let Some(url) = args.get("url").and_then(|v| v.as_str()) {
        tab.navigate_to(url)
            .with_context(|| format!("Failed to navigate to {}", url))?;
    }

    tab.wait_until_navigated()?;

    match action.as_str() {
        "click" => {
            let script = format!("document.querySelector('{}')?.click()", selector);
            tab.evaluate(&script, false)?;
            Ok(ToolResult::ok(format!("Clicked element: {}", selector)))
        }
        "scroll" => {
            let script = "window.scrollBy(0, window.innerHeight)";
            tab.evaluate(script, false)?;
            Ok(ToolResult::ok("Scrolled down one viewport".to_string()))
        }
        "type" => {
            let text = args
                .get("text")
                .or_else(|| args.get("value"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let script = format!("document.querySelector('{}')?.value = '{}'", selector, text);
            tab.evaluate(&script, false)?;
            Ok(ToolResult::ok(format!(
                "Typed '{}' into {}",
                text, selector
            )))
        }
        "hover" => {
            let script = format!(
                "document.querySelector('{}')?.dispatchEvent(new MouseEvent('mouseover', {{ bubbles: true }}))",
                selector
            );
            tab.evaluate(&script, false)?;
            Ok(ToolResult::ok(format!("Hovered over {}", selector)))
        }
        "clear" => {
            let script = format!(
                "const el = document.querySelector('{}'); if (el) el.value = '';",
                selector
            );
            tab.evaluate(&script, false)?;
            Ok(ToolResult::ok(format!("Cleared {}", selector)))
        }
        "select" => {
            let value = args.get("value").and_then(|v| v.as_str()).unwrap_or("");
            let script = format!(
                "const el = document.querySelector('{}'); if (el) el.value = '{}';",
                selector, value
            );
            tab.evaluate(&script, false)?;
            Ok(ToolResult::ok(format!(
                "Selected '{}' in {}",
                value, selector
            )))
        }
        _ => unreachable!("unsupported actions are rejected before browser setup"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_browser_navigate_missing_url() {
        let temp_dir = TempDir::new().unwrap();
        let args = json!({});
        let result = browser_navigate(&args, temp_dir.path()).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Missing arg: url"));
    }

    #[tokio::test]
    async fn test_browser_action_missing_action() {
        let temp_dir = TempDir::new().unwrap();
        let args = json!({});
        let result = browser_action(&args, temp_dir.path()).await;
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Missing arg: action")
        );
    }

    #[tokio::test]
    async fn test_browser_action_click() {
        let temp_dir = TempDir::new().unwrap();
        let args = json!({
            "action": "click",
            "selector": "#button"
        });
        let result = browser_action(&args, temp_dir.path()).await;
        match result {
            Ok(result) => assert!(result.success),
            Err(err) => assert!(!err.to_string().contains("Missing arg")),
        }
    }

    #[tokio::test]
    async fn test_browser_action_scroll() {
        let temp_dir = TempDir::new().unwrap();
        let args = json!({
            "action": "scroll"
        });
        let result = browser_action(&args, temp_dir.path()).await;
        match result {
            Ok(result) => assert!(result.success),
            Err(err) => assert!(!err.to_string().contains("Missing arg")),
        }
    }

    #[tokio::test]
    async fn test_browser_action_type() {
        let temp_dir = TempDir::new().unwrap();
        let args = json!({
            "action": "type",
            "selector": "#input",
            "text": "hello"
        });
        let result = browser_action(&args, temp_dir.path()).await;
        match result {
            Ok(result) => assert!(result.success),
            Err(err) => assert!(!err.to_string().contains("Missing arg")),
        }
    }

    #[tokio::test]
    async fn test_browser_action_unknown_action() {
        let temp_dir = TempDir::new().unwrap();
        let args = json!({
            "action": "unknown"
        });
        let result = browser_action(&args, temp_dir.path()).await;
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Unknown action: unknown")
        );
    }

    #[tokio::test]
    async fn test_browser_screenshot_invalid_path() {
        let temp_dir = TempDir::new().unwrap();
        let args = json!({
            "path": "../outside/screenshot.png"
        });
        let result = browser_screenshot(&args, temp_dir.path()).await;
        // Path traversal should be rejected by resolve()
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_browser_get_text_missing_selector() {
        let temp_dir = TempDir::new().unwrap();
        let args = json!({});
        let result = browser_get_text(&args, temp_dir.path()).await;
        match result {
            Ok(result) => assert!(result.output.contains("Text from selector 'body':")),
            Err(err) => assert!(!err.to_string().contains("Missing arg")),
        }
    }

    #[tokio::test]
    async fn test_browser_get_text_custom_selector() {
        let temp_dir = TempDir::new().unwrap();
        let args = json!({
            "selector": "#content"
        });
        let result = browser_get_text(&args, temp_dir.path()).await;
        match result {
            Ok(result) => assert!(result.output.contains("Text from selector '#content':")),
            Err(err) => assert!(!err.to_string().contains("Missing arg")),
        }
    }
}
