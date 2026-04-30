use serde_json::Value;

#[derive(Debug, Clone)]
pub struct Turn {
    pub step: usize,
    pub thought: String,
    pub tool: String,
    pub args: Value,
    pub output: String,
    pub success: bool,
}

pub struct History {
    pub turns: Vec<Turn>,
    pub window: usize,
    pub max_turns: usize,
}

impl History {
    pub fn new(window: usize) -> Self {
        Self {
            turns: Vec::new(),
            window,
            max_turns: 100,
        }
    }

    pub fn push(&mut self, turn: Turn) {
        self.turns.push(turn);
        // Trim in batches to amortize the O(n) cost of shifting a Vec.
        if self.turns.len() > self.max_turns + 8 {
            self.turns.drain(0..8);
        }
    }

    /// Return the last `n` turns (excluding step-0 memory injections).
    pub fn recent_turns(&self, n: usize) -> Vec<&Turn> {
        self.turns
            .iter()
            .filter(|t| t.step > 0)
            .rev()
            .take(n)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect()
    }

    /// Format history for injection into the LLM prompt.
    /// - Turns older than `window` are collapsed to one-liners.
    /// - Recent `window` turns are shown in full.
    /// - Each tool output is capped to keep prompts informative without exploding.
    pub fn format(&self, max_output_chars: usize) -> String {
        if self.turns.is_empty() {
            return "(no previous steps)".to_string();
        }

        let total = self.turns.len();
        let cutoff = total.saturating_sub(self.window);
        let mut parts: Vec<String> = Vec::new();

        // Summarize old turns — include thought preview so weak models retain
        // context about WHY each step was taken, not just what tool was used.
        if cutoff > 0 {
            let summaries: Vec<String> = self.turns[..cutoff]
                .iter()
                .map(|t| {
                    let ok = if t.success { "✓" } else { "✗" };
                    let key_arg = key_arg_preview(&t.tool, &t.args);
                    let tool_label = if key_arg.is_empty() {
                        t.tool.clone()
                    } else {
                        format!("{}({})", t.tool, key_arg)
                    };
                    let thought_preview = first_line(&t.thought, 40);
                    let output_preview = first_line(&t.output, 60);
                    if thought_preview.is_empty() {
                        format!("  step {:>2} {ok} [{}] → {}", t.step, tool_label, output_preview)
                    } else {
                        format!(
                            "  step {:>2} {ok} [{}] — {} | {}",
                            t.step, tool_label, thought_preview, output_preview
                        )
                    }
                })
                .collect();
            parts.push(format!(
                "--- earlier steps (summarized) ---\n{}",
                summaries.join("\n")
            ));
        }

        // Full detail for recent turns
        for t in &self.turns[cutoff..] {
            let ok = if t.success { "✓" } else { "✗" };
            let output = if t.output.len() > max_output_chars {
                format!(
                    "{}\n...[truncated, {} chars total]",
                    &t.output.chars().take(max_output_chars).collect::<String>(),
                    t.output.len()
                )
            } else {
                t.output.clone()
            };
            parts.push(format!(
                "--- step {} ---\nThought: {}\nTool: {} {ok}\nArgs: {}\nOutput:\n{}",
                t.step,
                t.thought,
                t.tool,
                serde_json::to_string(&t.args).unwrap_or_default(),
                indent(&output, "  "),
            ));
        }

        parts.join("\n\n")
    }
}

/// Extract the most meaningful argument for a tool to display in collapsed history.
/// Returns a short string like "src/lib.rs" or "plan" or "cargo test".
fn key_arg_preview(tool: &str, args: &Value) -> String {
    let obj = match args.as_object() {
        Some(o) => o,
        None => return String::new(),
    };

    // For file tools — show the path
    if matches!(
        tool,
        "read_file"
            | "write_file"
            | "str_replace"
            | "str_replace_multi"
            | "open_file_region"
            | "outline"
            | "get_symbols"
            | "get_signature"
    ) {
        if let Some(path) = obj.get("path").and_then(|v| v.as_str()) {
            // Show only the filename, not the full path
            return std::path::Path::new(path)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(path)
                .to_string();
        }
    }

    // For memory tools — show the key
    if matches!(tool, "memory_read" | "memory_write" | "memory_delete") {
        if let Some(key) = obj.get("key").and_then(|v| v.as_str()) {
            return key.to_string();
        }
    }

    // For run_command — show the program
    if tool == "run_command" {
        if let Some(prog) = obj.get("program").and_then(|v| v.as_str()) {
            // Include first arg if present and short
            if let Some(args_arr) = obj.get("args").and_then(|v| v.as_array()) {
                if let Some(first) = args_arr.first().and_then(|v| v.as_str()) {
                    if first.len() <= 12 {
                        return format!("{prog} {first}");
                    }
                }
            }
            return prog.to_string();
        }
    }

    // For search tools — show the pattern
    if matches!(tool, "search_in_files" | "find_files" | "find_references") {
        for key in &["pattern", "name"] {
            if let Some(val) = obj.get(*key).and_then(|v| v.as_str()) {
                let trimmed = first_line(val, 20);
                if !trimmed.is_empty() {
                    return trimmed;
                }
            }
        }
    }

    // For web tools — show the URL or query (trimmed)
    if matches!(tool, "fetch_url" | "web_search" | "browser_get_text" | "browser_navigate") {
        for key in &["url", "query"] {
            if let Some(val) = obj.get(*key).and_then(|v| v.as_str()) {
                return first_line(val, 30);
            }
        }
    }

    // For spawn_agent — show the role
    if tool == "spawn_agent" {
        if let Some(role) = obj.get("role").and_then(|v| v.as_str()) {
            return role.to_string();
        }
    }

    String::new()
}

fn first_line(s: &str, max: usize) -> String {
    let line = s.lines().next().unwrap_or("").trim();
    let mut chars = line.chars();
    let collected: String = chars.by_ref().take(max).collect();
    if chars.next().is_some() {
        format!("{collected}…")
    } else {
        collected
    }
}

fn indent(s: &str, prefix: &str) -> String {
    s.lines()
        .map(|l| format!("{prefix}{l}"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_turn(step: usize, tool: &str, args: Value, thought: &str, output: &str, success: bool) -> Turn {
        Turn {
            step,
            thought: thought.to_string(),
            tool: tool.to_string(),
            args,
            output: output.to_string(),
            success,
        }
    }

    #[test]
    fn collapsed_summary_includes_thought_preview() {
        // Guards against regressions that remove thought from the collapsed line.
        // Weak models need the thought context to understand WHY a step was taken.
        let mut history = History::new(1); // window=1 → all but last step collapse
        history.push(make_turn(
            1, "read_file",
            json!({ "path": "src/lib.rs" }),
            "Need to understand the module structure",
            "pub mod agent;",
            true,
        ));
        history.push(make_turn(
            2, "write_file",
            json!({ "path": "src/lib.rs" }),
            "Adding new module declaration",
            "Written successfully",
            true,
        ));

        let formatted = history.format(6000);
        // Step 1 should be collapsed and include the thought
        assert!(
            formatted.contains("Need to understand"),
            "collapsed step must include thought preview: {formatted}"
        );
        assert!(
            formatted.contains("lib.rs"),
            "collapsed step must include key arg: {formatted}"
        );
    }

    #[test]
    fn collapsed_summary_includes_key_arg_for_memory_tools() {
        let mut history = History::new(1);
        history.push(make_turn(
            1, "memory_read",
            json!({ "key": "knowledge/decisions" }),
            "Loading architectural decisions",
            "## Decision: use str_replace",
            true,
        ));
        history.push(make_turn(
            2, "write_file",
            json!({ "path": "src/lib.rs" }),
            "Implementing the change",
            "ok",
            true,
        ));

        let formatted = history.format(6000);
        assert!(
            formatted.contains("knowledge/decisions"),
            "memory_read collapsed line must show key: {formatted}"
        );
    }

    #[test]
    fn full_detail_turns_still_show_complete_thought() {
        let mut history = History::new(2);
        history.push(make_turn(
            1, "read_file",
            json!({ "path": "src/main.rs" }),
            "This is a very long thought that should appear in full in the detailed view",
            "fn main() {}",
            true,
        ));

        let formatted = history.format(6000);
        assert!(
            formatted.contains("This is a very long thought that should appear in full"),
            "full-detail turn must preserve complete thought"
        );
    }
}
