//use super::core::{str_arg, ToolResult};
use anyhow::{Result, bail};
use serde_json::Value;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};

use crate::agent::core::SweAgent;
use crate::config_struct::{AgentConfig, Role};
use crate::tools::core::{ToolResult, str_arg};
use crate::tui::TuiEvent;

// ─── TUI sender for sub-agent progress events ────────────────────────────────
// Set by the agent loop before dispatching spawn_agent; cleared after.
// Allows spawn_agent to notify the parent's TUI without carrying a typed
// channel through AgentConfig (which must remain Clone + Serialize).

type TuiSender = tokio::sync::mpsc::UnboundedSender<TuiEvent>;

fn tui_sender() -> &'static Mutex<Option<TuiSender>> {
    static TUI_TX: OnceLock<Mutex<Option<TuiSender>>> = OnceLock::new();
    TUI_TX.get_or_init(|| Mutex::new(None))
}

/// Install the parent agent's TUI sender so spawn_agent can forward events.
/// Called by the agent loop before dispatching; pass None to clear.
pub fn set_tui_sender(tx: Option<Arc<TuiSender>>) {
    *tui_sender().lock().expect("tui_sender mutex poisoned") = tx.map(|a| (*a).clone());
}

fn tui_send(ev: TuiEvent) {
    if let Some(tx) = tui_sender()
        .lock()
        .expect("tui_sender mutex poisoned")
        .as_ref()
    {
        let _ = tx.send(ev);
    }
}

/// Run a sub-agent to completion and return its result summary.
pub async fn spawn_agent(
    args: &Value,
    root: &Path,
    depth: usize,
    cfg: &AgentConfig,
) -> Result<ToolResult> {
    if depth >= cfg.max_depth {
        bail!(
            "spawn_agent refused: maximum nesting depth ({}) reached",
            cfg.max_depth
        );
    }

    let role_name = str_arg(args, "role")?;
    let task = str_arg(args, "task")?;
    let memory_key = args
        .get("memory_key")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let max_steps = args
        .get("max_steps")
        .and_then(|v| v.as_u64())
        .map(|n| n as usize)
        .unwrap_or_else(|| cfg.max_steps_for_role(&role_name));

    let role = match Role::role_from_str(&role_name) {
        Some(r) => r,
        None => bail!("Unknown role: {role_name}"),
    };

    if matches!(role, Role::Boss) {
        bail!("spawn_agent refused: boss cannot spawn another boss");
    }

    // If a memory_key is specified, append an explicit instruction so the sub-agent
    // knows it must call memory_write before finishing. Without this the key stays
    // empty and the boss cannot read the result.
    let enriched_task = if let Some(ref key) = memory_key {
        format!(
            "{task}\n\n\
            IMPORTANT: When you have finished, you MUST call memory_write with \
            key=\"{key}\" and your result summary as content. \
            Do not call finish without writing this key first."
        )
    } else {
        task.clone()
    };

    let root_str = root.to_string_lossy().to_string();

    tracing::info!(
        "[depth={depth}] spawning sub-agent: role={role_name}, steps={max_steps}, key={:?}",
        memory_key
    );
    let mut sub_agent =
        SweAgent::new_with_depth(cfg.clone(), &root_str, max_steps, role, depth + 1)?;

    let task_preview: String = {
        let first = enriched_task.lines().next().unwrap_or("").trim();
        let mut chars = first.chars();
        let collected: String = chars.by_ref().take(60).collect();
        if chars.next().is_some() {
            format!("{collected}…")
        } else {
            collected
        }
    };
    tui_send(TuiEvent::SubAgentSpawned {
        role: role_name.clone(),
        task_preview,
        depth: depth + 1,
    });

    sub_agent.run(&enriched_task, None, None).await?;

    tui_send(TuiEvent::SubAgentFinished {
        role: role_name.clone(),
        depth: depth + 1,
    });

    let result_text: String = if let Some(ref key) = memory_key {
        let read_args = serde_json::json!({ "key": key });
        match crate::tools::memory::memory_read(&read_args, root).await {
            Ok(r) if r.success && !r.output.is_empty() => {
                format!(
                    "Sub-agent ({role_name}) completed. Result stored in '{key}':\n{}",
                    r.output
                )
            }
            _ => format!(
                "Sub-agent ({role_name}) completed. Key '{key}' is empty or unwritten — \
                the sub-agent may have finished without calling memory_write."
            ),
        }
    } else {
        format!("Sub-agent ({role_name}) completed (no memory_key).")
    };

    Ok(ToolResult::ok(result_text))
}

/// Run multiple sub-agents sequentially, collecting all results.
///
/// NOTE: True parallelism requires SweAgent to implement Send.
/// Until then agents run one after another — interface identical to parallel.
pub async fn spawn_agents(
    args: &Value,
    root: &Path,
    depth: usize,
    cfg: &AgentConfig,
) -> Result<ToolResult> {
    if depth >= cfg.max_depth {
        bail!(
            "spawn_agents refused: maximum nesting depth ({}) reached",
            cfg.max_depth
        );
    }

    let agents_arr = args
        .get("agents")
        .and_then(|v| v.as_array())
        .ok_or_else(|| anyhow::anyhow!("Missing arg: agents (expected array)"))?;

    if agents_arr.is_empty() {
        bail!("spawn_agents: agents array is empty");
    }

    let mut lines: Vec<String> = Vec::new();
    let mut any_err = false;

    for (i, entry) in agents_arr.iter().enumerate() {
        let role_name: String = entry
            .get("role")
            .and_then(|v| v.as_str())
            .unwrap_or("developer")
            .to_string();
        let task: String = entry
            .get("task")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let memory_key: Option<String> = entry
            .get("memory_key")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let max_steps: usize = entry
            .get("max_steps")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize)
            .unwrap_or_else(|| cfg.max_steps_for_role(&role_name));

        if task.is_empty() {
            lines.push(format!("  agent[{i}] {role_name} → ERR: missing task"));
            any_err = true;
            continue;
        }

        let role = match Role::role_from_str(&role_name) {
            Some(r) => r,
            None => {
                lines.push(format!("  agent[{i}] {role_name} → ERR: unknown role"));
                any_err = true;
                continue;
            }
        };

        if matches!(role, Role::Boss) {
            lines.push(format!(
                "  agent[{i}] {role_name} → ERR: boss cannot be a sub-agent"
            ));
            any_err = true;
            continue;
        }

        let sub_args = serde_json::json!({
            "role": role_name, "task": task,
            "memory_key": memory_key, "max_steps": max_steps,
        });

        match spawn_agent(&sub_args, root, depth, cfg).await {
            Ok(result) => {
                let preview: &str = result.output.lines().next().unwrap_or("(empty)");
                let key_label: &str = memory_key.as_deref().unwrap_or("(no key)");
                lines.push(format!(
                    "  agent[{i}] {role_name} → key={key_label}: {preview}"
                ));
            }
            Err(e) => {
                lines.push(format!("  agent[{i}] {role_name} → ERR: {e}"));
                any_err = true;
            }
        }
    }

    let summary = format!(
        "Ran {} sub-agents (depth={}):\n{}",
        agents_arr.len(),
        depth + 1,
        lines.join("\n")
    );

    if any_err {
        Ok(ToolResult::ok(format!("[partial errors]\n{summary}")))
    } else {
        Ok(ToolResult::ok(summary))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config_struct::AgentConfig;
    use serde_json::json;

    fn cfg() -> AgentConfig {
        AgentConfig::default()
    }
    fn root() -> std::path::PathBuf {
        std::env::current_dir().unwrap()
    }

    #[tokio::test]
    async fn test_spawn_agent_missing_role() {
        let r = spawn_agent(&json!({ "task": "x" }), &root(), 0, &cfg()).await;
        assert!(r.is_err());
        assert!(r.unwrap_err().to_string().contains("role"));
    }

    #[tokio::test]
    async fn test_spawn_agent_missing_task() {
        let r = spawn_agent(&json!({ "role": "developer" }), &root(), 0, &cfg()).await;
        assert!(r.is_err());
        assert!(r.unwrap_err().to_string().contains("task"));
    }

    #[tokio::test]
    async fn test_spawn_agent_depth_limit() {
        let mut c = cfg();
        c.max_depth = 1;
        let r = spawn_agent(&json!({ "role": "developer", "task": "x" }), &root(), 1, &c).await;
        assert!(r.is_err());
        assert!(r.unwrap_err().to_string().contains("maximum nesting depth"));
    }

    #[tokio::test]
    async fn test_spawn_agent_boss_guard() {
        let r = spawn_agent(&json!({ "role": "boss", "task": "x" }), &root(), 0, &cfg()).await;
        assert!(r.is_err());
        assert!(
            r.unwrap_err()
                .to_string()
                .contains("boss cannot spawn another boss")
        );
    }

    #[tokio::test]
    async fn test_spawn_agent_unknown_role() {
        let r = spawn_agent(
            &json!({ "role": "hacker", "task": "x" }),
            &root(),
            0,
            &cfg(),
        )
        .await;
        assert!(r.is_err());
        assert!(r.unwrap_err().to_string().contains("Unknown role"));
    }

    #[tokio::test]
    async fn test_spawn_agents_missing_agents() {
        let r = spawn_agents(&json!({ "timeout_secs": 60 }), &root(), 0, &cfg()).await;
        assert!(r.is_err());
        assert!(r.unwrap_err().to_string().contains("agents"));
    }

    #[tokio::test]
    async fn test_spawn_agents_empty() {
        let r = spawn_agents(&json!({ "agents": [] }), &root(), 0, &cfg()).await;
        assert!(r.is_err());
        assert!(r.unwrap_err().to_string().contains("empty"));
    }

    #[tokio::test]
    async fn test_spawn_agents_depth_limit() {
        let mut c = cfg();
        c.max_depth = 1;
        let r = spawn_agents(
            &json!({ "agents": [{ "role": "developer", "task": "x" }] }),
            &root(),
            1,
            &c,
        )
        .await;
        assert!(r.is_err());
        assert!(r.unwrap_err().to_string().contains("maximum nesting depth"));
    }
}
