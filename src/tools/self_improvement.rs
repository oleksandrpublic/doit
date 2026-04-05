use super::core::ToolResult;
use anyhow::Result;
use serde_json::Value;
use std::path::Path;

pub async fn tool_request(args: &Value, _root: &Path) -> Result<ToolResult> {
    let capability = args
        .get("capability")
        .or_else(|| args.get("name"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| anyhow::anyhow!("Missing arg: capability"))?;
    let description = args
        .get("description")
        .and_then(|v| v.as_str())
        .unwrap_or("No description provided");
    let motivation = args
        .get("motivation")
        .and_then(|v| v.as_str())
        .unwrap_or("No motivation provided");
    Ok(ToolResult::ok(format!(
        "Requested new tool capability: {}\nDescription: {}\nMotivation: {}",
        capability, description, motivation
    )))
}

pub async fn capability_gap(args: &Value, _root: &Path) -> Result<ToolResult> {
    let context = args
        .get("task")
        .or_else(|| args.get("context"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| anyhow::anyhow!("Missing arg: task"))?;
    let impact = args
        .get("impact")
        .and_then(|v| v.as_str())
        .unwrap_or("No impact provided");
    Ok(ToolResult::ok(format!(
        "Analyzing capability gaps for task: {}\nImpact: {}",
        context, impact
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::path::PathBuf;

    #[tokio::test]
    async fn test_tool_request_success() {
        let args = json!({
            "capability": "code_review",
            "description": "Review code for best practices"
        });
        let root = PathBuf::from("/tmp");
        let result = tool_request(&args, &root).await.unwrap();
        assert!(result.success);
        assert!(result
            .output
            .contains("Requested new tool capability: code_review"));
        assert!(result
            .output
            .contains("Description: Review code for best practices"));
    }

    #[tokio::test]
    async fn test_tool_request_missing_capability() {
        let args = json!({});
        let root = PathBuf::from("/tmp");
        let result = tool_request(&args, &root).await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Missing arg: capability"));
    }

    #[tokio::test]
    async fn test_tool_request_default_description() {
        let args = json!({
            "capability": "testing"
        });
        let root = PathBuf::from("/tmp");
        let result = tool_request(&args, &root).await.unwrap();
        assert!(result.success);
        assert!(result
            .output
            .contains("Description: No description provided"));
    }

    #[tokio::test]
    async fn test_capability_gap_success() {
        let args = json!({
            "task": "implement_auth"
        });
        let root = PathBuf::from("/tmp");
        let result = capability_gap(&args, &root).await.unwrap();
        assert!(result.success);
        assert!(result
            .output
            .contains("Analyzing capability gaps for task: implement_auth"));
    }

    #[tokio::test]
    async fn test_capability_gap_missing_task() {
        let args = json!({});
        let root = PathBuf::from("/tmp");
        let result = capability_gap(&args, &root).await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Missing arg: task"));
    }
}
