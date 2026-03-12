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
    turns: Vec<Turn>,
    window: usize,
}

impl History {
    pub fn new(window: usize) -> Self {
        Self { turns: Vec::new(), window }
    }

    pub fn push(&mut self, turn: Turn) {
        self.turns.push(turn);
    }

    /// Format history for injection into the LLM prompt.
    /// - Turns older than `window` are collapsed to one-liners.
    /// - Recent `window` turns are shown in full.
    pub fn format(&self) -> String {
        if self.turns.is_empty() {
            return "(no previous steps)".to_string();
        }

        let total = self.turns.len();
        let cutoff = total.saturating_sub(self.window);
        let mut parts: Vec<String> = Vec::new();

        // Summarize old turns
        if cutoff > 0 {
            let summaries: Vec<String> = self.turns[..cutoff]
                .iter()
                .map(|t| {
                    let ok = if t.success { "✓" } else { "✗" };
                    let short = first_line(&t.output, 80);
                    format!("  step {:>2} {ok} [{}] → {}", t.step, t.tool, short)
                })
                .collect();
            parts.push(format!("--- earlier steps (summarized) ---\n{}", summaries.join("\n")));
        }

        // Full detail for recent turns
        for t in &self.turns[cutoff..] {
            let ok = if t.success { "✓" } else { "✗" };
            parts.push(format!(
                "--- step {} ---\nThought: {}\nTool: {} {ok}\nArgs: {}\nOutput:\n{}",
                t.step,
                t.thought,
                t.tool,
                serde_json::to_string(&t.args).unwrap_or_default(),
                indent(&t.output, "  "),
            ));
        }

        parts.join("\n\n")
    }
}

fn first_line(s: &str, max: usize) -> String {
    let line = s.lines().next().unwrap_or("").trim();
    if line.len() > max { format!("{}…", &line[..max]) } else { line.to_string() }
}

fn indent(s: &str, prefix: &str) -> String {
    s.lines().map(|l| format!("{prefix}{l}")).collect::<Vec<_>>().join("\n")
}
