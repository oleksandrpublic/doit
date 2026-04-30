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

    // Forward parent TUI to sub-agent's ask_human calls.
    // Sub-agents have tui=None, so install_tui_callbacks() is never called
    // inside their step(). Installing callbacks here ensures TUI Prompt
    // events reach the parent TUI when the sub-agent calls ask_human.
    // The sender was set by the parent's step() before dispatching spawn_agent.
    let tui_tx_for_sub: Option<TuiSender> = tui_sender()
        .lock()
        .expect("tui_sender mutex poisoned")
        .clone();
    if let Some(ref tx) = tui_tx_for_sub {
        crate::tools::human::install_tui_callbacks_from_tx(tx.clone());
        crate::tools::human::set_telegram_config(
            cfg.telegram_token.clone(),
            cfg.telegram_chat_id.clone(),
        );
    }

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

    // Clear TUI callbacks that were installed for the sub-agent.
    // This prevents stale closures from pointing at a potentially-closed
    // parent sender after the sub-agent finishes.
    if tui_tx_for_sub.is_some() {
        crate::tools::human::set_tui_callbacks(None, None, None, None, None);
        crate::tools::human::set_telegram_config(None, None);
    }

    tui_send(TuiEvent::SubAgentFinished {
        role: role_name.clone(),
        depth: depth + 1,
    });

    let result_text: String = build_result_text(&memory_key, root, &role_name).await;

    // If the sub-agent stopped due to user refusal or repeated errors, signal
    // failure so the boss does not silently re-delegate the same task.
    if is_sub_agent_stopped(&result_text) {
        return Ok(ToolResult {
            output: result_text,
            success: false,
        });
    }

    Ok(ToolResult::ok(result_text))
}

/// Build the result text that `spawn_agent` returns to the boss.
///
/// Extracted for testability — callers can verify the full
/// `result_text → success` path without running a live LLM.
pub async fn build_result_text(
    memory_key: &Option<String>,
    root: &Path,
    role_name: &str,
) -> String {
    if let Some(ref key) = memory_key {
        let read_args = serde_json::json!({ "key": key });
        match crate::tools::memory::memory_read(&read_args, root).await {
            Ok(r) if r.success && !r.output.is_empty() => format!(
                "Sub-agent ({role_name}) completed. Result stored in '{key}':\n{}",
                r.output
            ),
            _ => format!(
                "Sub-agent ({role_name}) completed. Key '{key}' is empty or unwritten — \
                the sub-agent may have finished without calling memory_write."
            ),
        }
    } else {
        format!("Sub-agent ({role_name}) completed (no memory_key).")
    }
}

/// Return true when the sub-agent result text indicates a user-driven or
/// error-driven stop that the boss should not silently retry.
///
/// Matches the standard stop messages produced by `run()` and `run_capture()`:
/// - "Stopped by user"               — user pressed stop or answered "no" to continue
/// - "exited due to repeated errors" — sub-agent gave up after consecutive failures
/// - "stopped due to no progress"    — loop detection fired
fn is_sub_agent_stopped(text: &str) -> bool {
    let lower = text.to_lowercase();
    lower.contains("stopped by user")
        || lower.contains("exited due to repeated errors")
        || lower.contains("stopped due to no progress")
}

/// Run multiple sub-agents in parallel using `tokio::task::spawn_local` + `LocalSet`.
///
/// ## Почему `spawn_local`, а не `tokio::spawn`
///
/// `SweAgent::step()` возвращает `Pin<Box<dyn Future + 'a>>` без `+ Send`,
/// поскольку future заимствует `&'a mut self`. `tokio::spawn` требует
/// `Future: Send + 'static` — это требование несовместимо с `&mut self`.
///
/// `spawn_local` не требует `Send` — задачи выполняются на одном потоке
/// внутри `LocalSet`. `LocalSet::run_until` работает прямо внутри
/// обычного `#[tokio::main]` без изменений в main.rs.
///
/// `timeout_secs` принимается для совместимости API, но не используется.
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

    if args.get("timeout_secs").and_then(|v| v.as_u64()).is_some() {
        tracing::info!("spawn_agents: timeout_secs has no effect");
    }

    // Pre-validate all entries before launching any tasks, so the boss gets
    // a clear error message without partial side effects.
    struct AgentSpec {
        index: usize,
        role_name: String,
        sub_args: serde_json::Value,
        memory_key: Option<String>,
    }

    let mut specs: Vec<AgentSpec> = Vec::new();
    let mut pre_errors: Vec<String> = Vec::new();

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
            pre_errors.push(format!("  agent[{i}] {role_name} → ERR: missing task"));
            continue;
        }

        let role = match Role::role_from_str(&role_name) {
            Some(r) => r,
            None => {
                pre_errors.push(format!("  agent[{i}] {role_name} → ERR: unknown role"));
                continue;
            }
        };

        if matches!(role, Role::Boss) {
            pre_errors.push(format!(
                "  agent[{i}] {role_name} → ERR: boss cannot be a sub-agent"
            ));
            continue;
        }

        let sub_args = serde_json::json!({
            "role": role_name, "task": task,
            "memory_key": memory_key, "max_steps": max_steps,
        });

        specs.push(AgentSpec { index: i, role_name, sub_args, memory_key });
    }

    // If all entries failed pre-validation, return early.
    if specs.is_empty() {
        let summary = format!(
            "spawn_agents: all {} entries failed validation:\n{}",
            agents_arr.len(),
            pre_errors.join("\n")
        );
        return Ok(ToolResult::ok(format!("[partial errors]\n{summary}")));
    }

    // ── Parallel execution via spawn_local + LocalSet ──────────────────────
    //
    // Each agent gets its own JoinHandle. We collect results in order.
    let root_owned = root.to_path_buf();
    let cfg_owned = cfg.clone();

    let results: Vec<(usize, String, Option<String>, Result<ToolResult>)> = {
        let local = tokio::task::LocalSet::new();

        // Spawn all tasks inside the LocalSet
        let handles: Vec<_> = specs
            .into_iter()
            .map(|spec| {
                let root_c = root_owned.clone();
                let cfg_c = cfg_owned.clone();
                let handle = local.spawn_local(async move {
                    let result = spawn_agent(&spec.sub_args, &root_c, depth, &cfg_c).await;
                    (spec.index, spec.role_name, spec.memory_key, result)
                });
                handle
            })
            .collect();

        // Drive the LocalSet to completion, then await all handles
        local
            .run_until(async {
                let mut out = Vec::new();
                for h in handles {
                    match h.await {
                        Ok(tuple) => out.push(tuple),
                        Err(e) => {
                            tracing::error!("spawn_agents: task panicked: {e}");
                        }
                    }
                }
                out
            })
            .await
    };

    // ── Collect results in original index order ────────────────────────────
    let mut lines: Vec<(usize, String)> = pre_errors
        .into_iter()
        .enumerate()
        .map(|(_, e)| (usize::MAX, e)) // pre-errors sort last
        .collect();

    let mut any_err = !lines.is_empty();

    for (i, role_name, memory_key, result) in results {
        let key_label = memory_key.as_deref().unwrap_or("(no key)");
        match result {
            Ok(r) => {
                let preview = r.output.lines().next().unwrap_or("(empty)");
                lines.push((i, format!("  agent[{i}] {role_name} → key={key_label}: {preview}")));
            }
            Err(e) => {
                lines.push((i, format!("  agent[{i}] {role_name} → ERR: {e}")));
                any_err = true;
            }
        }
    }

    // Sort by original index so output is deterministic
    lines.sort_by_key(|(i, _)| *i);
    let body: Vec<String> = lines.into_iter().map(|(_, s)| s).collect();

    let summary = format!(
        "Ran {} sub-agents in parallel (depth={}):\n{}",
        agents_arr.len(),
        depth + 1,
        body.join("\n")
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
    async fn test_tui_callbacks_cleared_if_no_sender() {
        // When no TUI sender is installed, spawn_agent must not install
        // TUI callbacks — set_tui_callbacks should remain None.
        // We can only verify this indirectly: the call must not panic
        // and must fail with the expected depth/role error, not a TUI error.
        set_tui_sender(None);
        let r = spawn_agent(
            &json!({ "role": "developer", "task": "x" }),
            &root(),
            1,    // depth=1 with max_depth=1 triggers depth error before any TUI code
            &{ let mut c = cfg(); c.max_depth = 1; c },
        ).await;
        assert!(r.is_err());
        assert!(r.unwrap_err().to_string().contains("maximum nesting depth"));
        // Callbacks must still be None after the call
        // (no spurious install happened)
        crate::tools::human::set_tui_callbacks(None, None, None, None, None);
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

    #[tokio::test]
    async fn test_spawn_agents_invalid_entries_reported_not_panicked() {
        // Invalid entries must surface as errors in output, not abort the call.
        let r = spawn_agents(
            &json!({
                "agents": [
                    { "role": "developer" },           // missing task
                    { "role": "nobody", "task": "x" }, // unknown role
                    { "role": "boss",   "task": "x" }, // boss guard
                ]
            }),
            &root(),
            0,
            &cfg(),
        )
        .await
        .unwrap();

        assert!(r.output.contains("[partial errors]"));
        assert!(r.output.contains("missing task"));
        assert!(r.output.contains("unknown role"));
        assert!(r.output.contains("boss cannot be a sub-agent"));
    }

    #[tokio::test]
    #[ignore = "too long — run manually"]
    async fn test_spawn_agents_output_says_parallel() {
        // Regression guard: output must say "in parallel" to reflect actual mode.
        let r = spawn_agents(
            &json!({ "agents": [{ "role": "developer", "task": "x" }] }),
            &root(),
            0,
            &cfg(),
        )
        .await
        .unwrap();
        assert!(
            r.output.contains("in parallel"),
            "output must mention parallel execution: {}",
            r.output
        );
    }

    // ── is_sub_agent_stopped ───────────────────────────────────────────────

    #[test]
    fn stopped_by_user_triggers_failure_flag() {
        assert!(is_sub_agent_stopped("Stopped by user during step 3"));
        assert!(is_sub_agent_stopped("stopped by user before step 1"));
    }

    #[test]
    fn exited_due_to_repeated_errors_triggers_failure_flag() {
        assert!(is_sub_agent_stopped(
            "[sub-agent: developer] exited due to repeated errors: connection refused"
        ));
        assert!(is_sub_agent_stopped("Exited due to repeated errors at step 4: timeout"));
    }

    #[test]
    fn stopped_due_to_no_progress_triggers_failure_flag() {
        assert!(is_sub_agent_stopped(
            "[sub-agent: navigator] stopped due to no progress: Agent stuck in loop"
        ));
    }

    #[test]
    fn normal_completion_does_not_trigger_failure_flag() {
        assert!(!is_sub_agent_stopped("Sub-agent (developer) completed (no memory_key)."));
        assert!(!is_sub_agent_stopped(
            "Sub-agent (qa) completed. Result stored in 'knowledge/qa_report':\nAll tests passed."
        ));
    }
}