use crate::agent::core::SweAgent;
use crate::agent::tools::{first_line, format_args_display};
use crate::tui::TuiEvent;
use anyhow::Result;

impl SweAgent {
    pub(crate) fn console_output_summary(
        &self,
        canonical_tool: &str,
        args: &serde_json::Value,
        output: &str,
    ) -> String {
        match canonical_tool {
            "read_file" | "open_file_region" => {
                let path = args.get("path").and_then(|v| v.as_str()).unwrap_or("?");
                let name = std::path::Path::new(path)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or(path);
                // First output line often contains range info, e.g. "File: foo.rs (lines 1-80 of 240)"
                let detail = output.lines().next().unwrap_or("").trim();
                let detail = detail
                    .trim_start_matches("File: ")
                    .trim_start_matches(name)
                    .trim();
                if detail.is_empty() {
                    let lines = output.lines().count();
                    format!("{name} ({lines} lines)")
                } else {
                    format!("{name} {detail}")
                }
            }
            "write_file" | "str_replace" => {
                let path = args.get("path").and_then(|v| v.as_str()).unwrap_or("?");
                let name = std::path::Path::new(path)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or(path);
                format!("{name} written")
            }
            "memory_read" => {
                let key = args.get("key").and_then(|v| v.as_str()).unwrap_or("?");
                let lines = output.lines().count();
                format!("key={key} ({lines} lines)")
            }
            "memory_write" => {
                let key = args.get("key").and_then(|v| v.as_str()).unwrap_or("?");
                format!("key={key} updated")
            }
            "fetch_url" | "browser_get_text" => {
                let lines = output.lines().count();
                format!("content loaded ({lines} lines)")
            }
            "spawn_agent" | "spawn_agents" => first_line(output, 100),
            "list_dir" => {
                let path = args.get("path").and_then(|v| v.as_str()).unwrap_or(".");
                let entries = output.lines().filter(|l| !l.trim().is_empty()).count();
                format!("{path} ({entries} entries)")
            }
            "find_files" => {
                let count = output.lines().filter(|l| !l.trim().is_empty()).count();
                format!("{count} files found")
            }
            "search_in_files" => {
                let count = output.lines().filter(|l| !l.trim().is_empty()).count();
                format!("{count} matches")
            }
            "tree" => {
                let lines = output.lines().count();
                format!("tree ({lines} lines)")
            }
            "project_map" => "project map loaded".to_string(),
            "outline" => {
                let path = args.get("path").and_then(|v| v.as_str()).unwrap_or("?");
                let name = std::path::Path::new(path)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or(path);
                let items = output.lines().filter(|l| !l.trim().is_empty()).count();
                format!("{name} ({items} items)")
            }
            "get_symbols" => {
                let count = output.lines().filter(|l| !l.trim().is_empty()).count();
                format!("{count} symbols")
            }
            "find_references" => {
                let count = output.lines().filter(|l| !l.trim().is_empty()).count();
                format!("{count} references")
            }
            "trace_call_path" => "call path loaded".to_string(),
            "run_command" => first_line(output, 90),
            "diff_repo" => {
                let lines = output.lines().count();
                format!("diff ({lines} lines)")
            }
            "notify" => "notification sent".to_string(),
            _ => first_line(output, 120),
        }
    }

    fn spawn_agent_args_preview(&self, args: &serde_json::Value) -> String {
        let role = args.get("role").and_then(|v| v.as_str()).unwrap_or("?");
        let key = args
            .get("memory_key")
            .and_then(|v| v.as_str())
            .unwrap_or("-");
        let task = args.get("task").and_then(|v| v.as_str()).unwrap_or("");
        let task_preview = if task.is_empty() {
            String::new()
        } else {
            self.spawn_agent_task_label(task)
        };

        if task_preview.is_empty() {
            format!("role={role} key={key}")
        } else {
            format!("role={role} key={key} task={task_preview}")
        }
    }

    fn spawn_agent_task_label(&self, task: &str) -> String {
        let line = first_line(task, 80);
        let lower = line.to_ascii_lowercase();

        let action = if lower.contains("read ")
            || lower.contains("inspect ")
            || lower.contains("review ")
            || lower.contains("map ")
            || lower.contains("summari")
            || lower.contains("analy")
            || lower.contains("understand ")
        {
            "inspect"
        } else if lower.contains("implement ")
            || lower.contains("build ")
            || lower.contains("create ")
            || lower.contains("fix ")
            || lower.contains("edit ")
            || lower.contains("write ")
        {
            "implement"
        } else if lower.contains("test ")
            || lower.contains("verify ")
            || lower.contains("check ")
            || lower.contains("validate ")
        {
            "verify"
        } else if lower.contains("search ")
            || lower.contains("find ")
            || lower.contains("research ")
            || lower.contains("look up")
        {
            "research"
        } else {
            "task"
        };

        if lower.contains("todo.md") {
            format!("{action} TODO.md")
        } else if lower.contains("task_s.txt") {
            format!("{action} task_s.txt")
        } else if lower.contains("requirements") {
            format!("{action} requirements")
        } else if lower.contains("plan") {
            format!("{action} plan")
        } else if lower.contains("tests") || lower.contains("test suite") {
            format!("{action} tests")
        } else if lower.contains("ui") || lower.contains("tui") {
            format!("{action} UI")
        } else {
            format!("{action} {}", first_line(&line, 36))
        }
    }

    pub(crate) fn spawn_agent_result_summary(
        &self,
        args: &serde_json::Value,
        output: &str,
    ) -> String {
        let role = args.get("role").and_then(|v| v.as_str()).unwrap_or("agent");
        let key = args.get("memory_key").and_then(|v| v.as_str());
        let task = args.get("task").and_then(|v| v.as_str()).unwrap_or("");
        let task_label = if task.is_empty() {
            role.to_string()
        } else {
            format!("{role} {}", self.spawn_agent_task_label(task))
        };

        // Detect outcome by checking for the sentinel phrases agents.rs uses.
        // Using dedicated constants avoids silent breakage if message text changes.
        const STORED_SENTINEL: &str = "Result stored in";
        const EMPTY_SENTINEL: &str = "is empty or unwritten";
        const NO_KEY_SENTINEL: &str = "no memory_key";

        if key.is_some() && output.contains(STORED_SENTINEL) {
            let key = key.unwrap();
            // Include a brief excerpt of the stored content if available
            let excerpt = output
                .split_once(":\n")
                .map(|(_, rest)| first_line(rest, 60))
                .unwrap_or_default();
            if excerpt.is_empty() {
                format!("{task_label} -> key={key} stored")
            } else {
                format!("{task_label} -> key={key} stored: {excerpt}")
            }
        } else if output.contains(EMPTY_SENTINEL) {
            if let Some(key) = key {
                format!("{task_label} -> key={key} empty")
            } else {
                format!("{task_label} -> no result")
            }
        } else if output.contains(NO_KEY_SENTINEL) {
            format!("{task_label} -> no key")
        } else {
            first_line(output, 100)
        }
    }

    pub(crate) fn tui_args_preview(
        &self,
        canonical_tool: &str,
        args: &serde_json::Value,
    ) -> String {
        match canonical_tool {
            "spawn_agent" => self.spawn_agent_args_preview(args),
            "spawn_agents" => self.boss_console_step_label(canonical_tool, args),
            "memory_read" | "memory_write" => {
                let key = args.get("key").and_then(|v| v.as_str()).unwrap_or("?");
                format!("key={key}")
            }
            "ask_human" => "prompt=(hidden)".to_string(),
            "notify" => "message=(hidden)".to_string(),
            "run_command" => {
                let cmd = args
                    .get("command")
                    .or_else(|| args.get("cmd"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("?");
                format!("command={}", first_line(cmd, 60))
            }
            _ => {
                let s = format_args_display(args);
                first_line(&s, 80)
            }
        }
    }

    fn boss_task_source_processed(&self) -> bool {
        self.history().turns.iter().any(|turn| {
            if turn.step == 0 {
                return false;
            }
            if turn.tool == "spawn_agent" {
                return turn
                    .args
                    .get("task")
                    .and_then(|v| v.as_str())
                    .zip(self.task_source())
                    .is_some_and(|(task, source)| task.contains(source));
            }
            if turn.tool == "spawn_agents" {
                return turn
                    .args
                    .get("agents")
                    .and_then(|v| v.as_array())
                    .is_some_and(|agents| {
                        self.task_source().is_some_and(|source| {
                            agents.iter().any(|agent| {
                                agent.get("role").and_then(|v| v.as_str()) == Some("navigator")
                                    && agent
                                        .get("task")
                                        .and_then(|v| v.as_str())
                                        .is_some_and(|task| task.contains(source))
                            })
                        })
                    });
            }
            false
        })
    }

    pub(crate) fn enforce_boss_task_source_priority(
        &self,
        canonical_tool: &str,
        args: &serde_json::Value,
    ) -> Result<()> {
        if self.role().name() != "boss" {
            return Ok(());
        }
        let Some(task_source) = self.task_source() else {
            return Ok(());
        };
        if self.boss_task_source_processed() {
            return Ok(());
        }

        let allowed_memory_keys = [
            "last_session",
            "plan",
            "knowledge/decisions",
            "user_profile",
            "boss_notes",
        ];

        match canonical_tool {
            "memory_read" => {
                let key = args.get("key").and_then(|v| v.as_str()).unwrap_or("");
                if allowed_memory_keys.contains(&key) {
                    Ok(())
                } else {
                    let msg = format!(
                        "boss guard: authoritative task source '{}' must be processed before memory key '{}'",
                        task_source, key
                    );
                    self.tui_send(TuiEvent::Status(msg.clone()));
                    if !crate::tui::tui_is_active() && self.depth() == 0 {
                        println!("           BLOCKED {msg}");
                    }
                    anyhow::bail!(
                        "Boss must process the authoritative task source '{}' before reading unrelated memory key '{}'",
                        task_source,
                        key
                    );
                }
            }
            "spawn_agent" => {
                let role = args.get("role").and_then(|v| v.as_str()).unwrap_or("");
                let task = args.get("task").and_then(|v| v.as_str()).unwrap_or("");
                if role == "navigator" && task.contains(task_source) {
                    Ok(())
                } else {
                    let msg = format!(
                        "boss guard: first delegation must inspect task source '{}'",
                        task_source
                    );
                    self.tui_send(TuiEvent::Status(msg.clone()));
                    if !crate::tui::tui_is_active() && self.depth() == 0 {
                        println!("           BLOCKED {msg}");
                    }
                    anyhow::bail!(
                        "Boss must first delegate a navigator to inspect the authoritative task source '{}'",
                        task_source
                    );
                }
            }
            "spawn_agents" => {
                let has_navigator_for_source = args
                    .get("agents")
                    .and_then(|v| v.as_array())
                    .is_some_and(|agents| {
                        agents.iter().any(|agent| {
                            agent.get("role").and_then(|v| v.as_str()) == Some("navigator")
                                && agent
                                    .get("task")
                                    .and_then(|v| v.as_str())
                                    .is_some_and(|task| task.contains(task_source))
                        })
                    });
                if has_navigator_for_source {
                    Ok(())
                } else {
                    let msg = format!(
                        "boss guard: first delegation batch must include navigator for '{}'",
                        task_source
                    );
                    self.tui_send(TuiEvent::Status(msg.clone()));
                    if !crate::tui::tui_is_active() && self.depth() == 0 {
                        println!("           BLOCKED {msg}");
                    }
                    anyhow::bail!(
                        "Boss must first delegate a navigator to inspect the authoritative task source '{}'",
                        task_source
                    );
                }
            }
            "tree" | "project_map" | "memory_write" | "notify" => Ok(()),
            "ask_human" => {
                let msg = format!(
                    "boss guard: do not ask human before delegating navigator for '{}'",
                    task_source
                );
                self.tui_send(TuiEvent::Status(msg.clone()));
                if !crate::tui::tui_is_active() && self.depth() == 0 {
                    println!("           BLOCKED {msg}");
                }
                anyhow::bail!(
                    "Boss must first delegate a navigator to inspect the authoritative task source '{}' before asking the human",
                    task_source
                );
            }
            "finish" => {
                let msg = format!(
                    "boss guard: cannot finish before processing task source '{}'",
                    task_source
                );
                self.tui_send(TuiEvent::Status(msg.clone()));
                if !crate::tui::tui_is_active() && self.depth() == 0 {
                    println!("           BLOCKED {msg}");
                }
                anyhow::bail!(
                    "Boss cannot finish before processing the authoritative task source '{}'",
                    task_source
                )
            }
            _ => Ok(()),
        }
    }

    pub(crate) fn boss_console_step_label(
        &self,
        canonical_tool: &str,
        args: &serde_json::Value,
    ) -> String {
        match canonical_tool {
            "spawn_agent" => self.spawn_agent_args_preview(args),
            "spawn_agents" => {
                let count = args
                    .get("agents")
                    .and_then(|v| v.as_array())
                    .map(|a| a.len())
                    .unwrap_or(0);
                format!("spawn_agents count={count}")
            }
            "memory_read" => {
                let key = args.get("key").and_then(|v| v.as_str()).unwrap_or("?");
                format!("memory_read key={key}")
            }
            "memory_write" => {
                let key = args.get("key").and_then(|v| v.as_str()).unwrap_or("?");
                format!("memory_write key={key}")
            }
            "notify" => "notify".to_string(),
            "ask_human" => "ask_human".to_string(),
            other => other.to_string(),
        }
    }
}
