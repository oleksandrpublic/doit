//! Shared result type for tool operations.

/// Result type for tool operations.
#[derive(Debug, Clone)]
pub struct ToolResult<T = String> {
    pub output: T,
    pub success: bool,
}

impl<T> ToolResult<T> {
    pub fn ok(output: impl Into<T>) -> Self {
        Self {
            output: output.into(),
            success: true,
        }
    }

    pub fn failure(output: impl Into<T>) -> Self {
        Self {
            output: output.into(),
            success: false,
        }
    }

    pub fn error(output: impl Into<T>) -> Self {
        Self {
            output: output.into(),
            success: false,
        }
    }
}

impl ToolResult<String> {
    pub fn into_anyhow(self) -> anyhow::Result<String> {
        if self.success {
            Ok(self.output)
        } else {
            Err(anyhow::anyhow!(self.output))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_into_anyhow_success() {
        let result = ToolResult::ok("success".to_string());
        assert!(result.into_anyhow().is_ok());
    }

    #[test]
    fn test_into_anyhow_error() {
        let result = ToolResult::failure("error".to_string());
        assert!(result.into_anyhow().is_err());
    }
}
