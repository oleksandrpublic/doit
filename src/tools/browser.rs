use super::core::{ToolResult, str_arg, resolve};
use anyhow::{Result, Context};
use headless_chrome::{Browser, LaunchOptions};
use serde_json::Value;
use std::path::Path;
use std::sync::Arc;

thread_local! {
    static BROWSER: std::cell::RefCell<Option<Arc<Browser>>> = std::cell::RefCell::new(None);
}

fn get_browser() -> Result<Arc<Browser>> {
    BROWSER.with(|cell| {
        let mut borrow = cell.borrow_mut();
        if borrow.is_none() {
            let browser = Browser::new(LaunchOptions {
                headless: true,
                ..Default::default()
            })?;
            *borrow = Some(Arc::new(browser));
        }
        Ok(borrow.as_ref().unwrap().clone())
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
    
    Ok(ToolResult::ok(format!("Successfully navigated to: {}", url)))
}

pub async fn browser_get_text(args: &Value, _root: &Path) -> Result<ToolResult> {
    let selector = args.get("selector").and_then(|v| v.as_str()).unwrap_or("body");
    
    let browser = get_browser()?;
    let tab = browser.new_tab()?;
    
    tab.wait_until_navigated()?;
    
    let script = format!("document.querySelector('{}')?.textContent || ''", selector);
    
    let result = tab.evaluate(&script, false)?;
    let text = result.value.and_then(|v| v.as_str().map(|s| s.to_string())).unwrap_or_default();
    
    Ok(ToolResult::ok(format!("Text from selector '{}': {}", selector, text)))
}

pub async fn browser_screenshot(args: &Value, root: &Path) -> Result<ToolResult> {
    let path = args.get("path").and_then(|v| v.as_str()).unwrap_or("screenshot.png");
    
    let full_path = resolve(root, path)?;
    
    let browser = get_browser()?;
    let tab = browser.new_tab()?;
    
    tab.wait_until_navigated()?;
    
    let png_data = tab.capture_screenshot(
        headless_chrome::protocol::cdp::Page::CaptureScreenshotFormatOption::Png,
        None,
        None,
        true,
    )?;
    
    std::fs::write(&full_path, png_data)
        .with_context(|| format!("Failed to write screenshot to {}", full_path.display()))?;
    
    Ok(ToolResult::ok(format!("Screenshot saved to: {}", full_path.display())))
}

pub async fn browser_action(args: &Value, _root: &Path) -> Result<ToolResult> {
    let action = str_arg(args, "action")?;
    let selector = args.get("selector").and_then(|v| v.as_str()).unwrap_or("body");
    
    let browser = get_browser()?;
    let tab = browser.new_tab()?;
    
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
            let text = args.get("text").and_then(|v| v.as_str()).unwrap_or("");
            let script = format!("document.querySelector('{}')?.value = '{}'", selector, text);
            tab.evaluate(&script, false)?;
            Ok(ToolResult::ok(format!("Typed '{}' into {}", text, selector)))
        }
        _ => Ok(ToolResult::ok(format!("Unknown action: {}", action))),
    }
}
