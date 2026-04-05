use crate::agent::core::{StepOutcome, StopReason, SweAgent};
use crate::agent::tools::{ParseActionError, ParseActionErrorKind, parse_action};
use crate::history::Turn;
use crate::tools::{self, ToolStatus, canonical_tool_name, find_tool_spec, tool_status};
use crate::tui::TuiEvent;
use crate::validation::resolve_safe_path;
use anyhow::Result;
use serde_json::Value;
use std::path::PathBuf;
use std::sync::Arc;

pub struct SessionArtifacts {
    pub log_path: PathBuf,
    pub trace_path: Option<PathBuf>,
    pub total_calls: usize,
    pub ok_calls: usize,
    pub err_calls: usize,
    pub started_at: std::time::Instant,
    pub started_at_str: String,
}

fn suppress_human_escalation_for_error(role_name: &str, err: &str) -> bool {
    if err.contains("Stopped by user") {
        return true;
    }

    if role_name == "boss"
        && (err.contains("Boss must first delegate")
            || err.contains("Boss must process the authoritative task source")
            || err.contains("Boss cannot finish before processing")
            || err.contains("before asking the human"))
    {
        return true;
    }

    false
}

fn build_reroute_message(
    user_message: &str,
    thinking_model: &str,
    target_model: &str,
    thought: &str,
    tool: &str,
    args: &Value,
    raw_response: &str,
) -> String {
    format!(
        "{user_message}\n\n### Prior Model Draft\n\
         The `{thinking_model}` model already analyzed this step and proposed a draft action.\n\
         Review it and produce the FINAL action for this same step.\n\n\
         Draft thought: {thought}\n\
         Draft tool: {tool}\n\
         Draft args: {args}\n\n\
         Raw draft response:\n\
         {raw_response}\n\n\
         ### Instructions\n\
         - Preserve the same task and constraints.\n\
         - Reuse the draft if it is good.\n\
         - Correct it if a better tool or narrower action is needed.\n\
         - Return exactly one final action in the normal agent format.\n\
         - Do not explain alternatives outside that action.\n\
         - You are the final decision maker for model `{target_model}`."
    )
}

async fn await_or_stop<T, F>(stop: Option<Arc<std::sync::atomic::AtomicBool>>, fut: F) -> Result<T>
where
    F: std::future::Future<Output = Result<T>>,
{
    let stop_future = async move {
        loop {
            if stop
                .as_ref()
                .is_some_and(|flag| flag.load(std::sync::atomic::Ordering::Relaxed))
            {
                anyhow::bail!("Stopped by user");
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    };

    tokio::pin!(fut);
    tokio::pin!(stop_future);

    tokio::select! {
        res = &mut fut => res,
        stop_res = &mut stop_future => stop_res,
    }
}

fn handle_parse_failure(
    agent: &mut SweAgent,
    err: ParseActionError,
    phase: &str,
) -> Result<StepOutcome> {
    agent.inc_consecutive_parse_failures();

    match err.kind() {
        ParseActionErrorKind::EmptyResponse => {
            tracing::warn!(
                "LLM returned empty response {phase} (consecutive failures: {})",
                agent.consecutive_parse_failures()
            );
            if agent.consecutive_parse_failures() >= 3 {
                anyhow::bail!(
                    "Agent stuck: {} consecutive empty parse responses",
                    agent.consecutive_parse_failures()
                );
            }
        }
        ParseActionErrorKind::MissingJson
        | ParseActionErrorKind::UnterminatedJson
        | ParseActionErrorKind::InvalidJson => {
            tracing::warn!(
                "LLM returned malformed action {phase} (consecutive failures: {}): {}",
                agent.consecutive_parse_failures(),
                err.detail()
            );
            if agent.consecutive_parse_failures() >= 2 {
                anyhow::bail!(
                    "Agent stuck: {} consecutive malformed actions ({})",
                    agent.consecutive_parse_failures(),
                    err.detail()
                );
            }
        }
    }

    Ok(StepOutcome::Continue)
}

impl SweAgent {
    pub async fn run(
        &mut self,
        task: &str,
        task_image: Option<PathBuf>,
        task_source: Option<String>,
    ) -> Result<()> {
        self.set_task_source(task_source);
        if let Err(e) = self.llm().check_models(&self.all_models()).await {
            tracing::warn!("{e}");
        }

        // TUI only at top-level; sub-agents skip it
        if self.depth() == 0 {
            if let Some(h) = crate::tui::start() {
                h.send(TuiEvent::SessionStarted {
                    task: task.to_string(),
                    role: self.role().name().to_string(),
                    max_steps: self.max_steps(),
                    repo: self.root().display().to_string(),
                });
                crate::tui::set_tui_active(true);
                self.set_tui(Some(h));
            } else {
                println!("\n╔══════════════════════════════════════╗");
                println!("║           do_it Agent Starting       ║");
                println!("╚══════════════════════════════════════╝");
                println!("Repo : {}", self.root().display());
                println!("Role : {}", self.role().name());
                println!("Steps: max {}", self.max_steps());
                println!("Models:");
                println!("  default   : {}", self.default_model());
                if let Some(m) = &self.router().thinking {
                    println!("  thinking  : {m}");
                }
                if let Some(m) = &self.router().coding {
                    println!("  coding    : {m}");
                }
                if let Some(m) = &self.router().search {
                    println!("  search    : {m}");
                }
                if let Some(m) = &self.router().execution {
                    println!("  execution : {m}");
                }
                if let Some(m) = &self.router().vision {
                    println!("  vision    : {m}");
                }
                println!();
            }
        }

        let session_started_at = std::time::Instant::now();
        let session_started_at_str = tools::chrono_now();
        self.session_init();

        if let Some(img) = &task_image {
            if let Err(e) = resolve_safe_path(self.root(), &img.to_string_lossy()) {
                anyhow::bail!("Task image path must be within root directory: {}", e);
            }
        }

        let effective_task = if let Some(img) = task_image {
            let vision_model = self.router().resolve(
                &crate::config_struct::ModelRole::Vision,
                self.default_model(),
            );
            if !crate::tui::tui_is_active() {
                println!("Task : [image] {}", img.display());
                println!("       Describing with [{vision_model}]...");
            }

            let stop = self.tui().map(|h| h.stop.clone());
            let description = await_or_stop(
                stop,
                self.llm().with_model(&vision_model).chat_with_image(
                    self.system_prompt(),
                    "Describe this image in detail. Focus on any code, diagrams, \
                     error messages, or UI elements. This description will be used \
                     as the task for a software engineering agent.",
                    &img,
                ),
            )
            .await?;

            if !crate::tui::tui_is_active() {
                println!(
                    "       -> {}\n",
                    description.lines().next().unwrap_or("(no description)")
                );
            }

            self.history_mut().push(Turn {
                step: 0,
                thought: "Analysing task image".to_string(),
                tool: "read_image".to_string(),
                args: serde_json::json!({ "path": img.display().to_string() }),
                output: description.clone(),
                success: true,
            });

            description
        } else {
            let resumed_task = self.resume_effective_task(task);
            if !crate::tui::tui_is_active() {
                println!("Task : {resumed_task}\n");
            }
            resumed_task
        };

        let mut consecutive_errors = 0;
        for step in 1..=self.max_steps() {
            if self.tui().is_some_and(|h| h.stop_requested()) {
                let summary = format!("Stopped by user before step {step}");
                let stop_reason = StopReason::Error;
                self.tui_send(TuiEvent::SessionFinished {
                    stop_reason,
                    summary_preview: summary.clone(),
                    steps_used: step.saturating_sub(1),
                });
                let artifacts = self.session_finish(
                    &effective_task,
                    &summary,
                    stop_reason,
                    step.saturating_sub(1),
                    session_started_at,
                    &session_started_at_str,
                );
                if self.depth() == 0 {
                    self.shutdown_tui();
                    self.print_final_summary(
                        stop_reason,
                        &summary,
                        step.saturating_sub(1),
                        artifacts.as_ref(),
                    );
                }
                return Ok(());
            }
            self.tui_send(TuiEvent::Status(format!(
                "step {}/{}",
                step,
                self.max_steps()
            )));

            match self.step(&effective_task, step).await {
                Ok(StepOutcome::Continue) => {
                    consecutive_errors = 0;
                }
                Ok(StepOutcome::Finished {
                    summary,
                    stop_reason,
                }) => {
                    self.tui_send(TuiEvent::SessionFinished {
                        stop_reason,
                        summary_preview: summary.lines().next().unwrap_or("").to_string(),
                        steps_used: step,
                    });
                    let artifacts = self.session_finish(
                        &effective_task,
                        &summary,
                        stop_reason,
                        step,
                        session_started_at,
                        &session_started_at_str,
                    );
                    if self.depth() == 0 {
                        self.shutdown_tui();
                        self.print_final_summary(stop_reason, &summary, step, artifacts.as_ref());
                    }
                    return Ok(());
                }
                Err(e) => {
                    if e.to_string().contains("Stopped by user") {
                        let summary = format!("Stopped by user during step {step}");
                        let stop_reason = StopReason::Error;
                        self.tui_send(TuiEvent::Status(summary.clone()));
                        self.tui_send(TuiEvent::SessionFinished {
                            stop_reason,
                            summary_preview: summary.clone(),
                            steps_used: step.saturating_sub(1),
                        });
                        let artifacts = self.session_finish(
                            &effective_task,
                            &summary,
                            stop_reason,
                            step.saturating_sub(1),
                            session_started_at,
                            &session_started_at_str,
                        );
                        if self.depth() == 0 {
                            self.shutdown_tui();
                            self.print_final_summary(
                                stop_reason,
                                &summary,
                                step.saturating_sub(1),
                                artifacts.as_ref(),
                            );
                        }
                        return Ok(());
                    }

                    if e.to_string().contains("Agent stuck in loop") {
                        let summary = format!("Stopped due to no progress at step {step}: {e}");
                        let stop_reason = StopReason::NoProgress;
                        self.tui_send(TuiEvent::Status("Stopped: no progress".into()));
                        self.tui_send(TuiEvent::SessionFinished {
                            stop_reason,
                            summary_preview: summary.clone(),
                            steps_used: step,
                        });
                        let artifacts = self.session_finish(
                            &effective_task,
                            &summary,
                            stop_reason,
                            step,
                            session_started_at,
                            &session_started_at_str,
                        );
                        if self.depth() == 0 {
                            self.shutdown_tui();
                            self.print_final_summary(
                                stop_reason,
                                &summary,
                                step,
                                artifacts.as_ref(),
                            );
                        }
                        return Ok(());
                    }

                    consecutive_errors += 1;
                    tracing::error!("Step {step} error: {e}");
                    if consecutive_errors >= 2
                        && !suppress_human_escalation_for_error(self.role().name(), &e.to_string())
                    {
                        let question = format!(
                            "Agent has encountered {} consecutive errors.\nLast error: {}\n\nContinue? (yes/no)",
                            consecutive_errors, e
                        );
                        let args = serde_json::json!({ "prompt": question, "timeout_secs": 300 });
                        // Suspend TUI for stdin input
                        if let Some(h) = self.tui() {
                            let tx1 = h.tx.clone();
                            let tx2 = h.tx.clone();
                            let tx3 = h.tx.clone();
                            tools::human::set_tui_callbacks(
                                Some(Arc::new(move || {
                                    let (ack_tx, ack_rx) = std::sync::mpsc::sync_channel(0);
                                    let _ = tx1.send(TuiEvent::Suspend(ack_tx));
                                    let _ = ack_rx.recv();
                                })),
                                Some(Arc::new(move || {
                                    let _ = tx2.send(TuiEvent::Resume);
                                })),
                                Some(Arc::new(|_msg: &str| {})),
                                Some(Arc::new(move |prompt: &str, timeout_secs: u64| {
                                    let (resp_tx, resp_rx) = std::sync::mpsc::sync_channel(1);
                                    let _ = tx3.send(TuiEvent::Prompt {
                                        prompt: prompt.to_string(),
                                        timeout_secs,
                                        response: resp_tx,
                                    });
                                    resp_rx.recv().ok().flatten()
                                })),
                            );
                        }
                        tools::human::set_telegram_config(
                            self.cfg_snapshot().telegram_token.clone(),
                            self.cfg_snapshot().telegram_chat_id.clone(),
                        );
                        match tools::dispatch_with_depth(
                            "ask_human",
                            &args,
                            self.root(),
                            self.depth(),
                            &[],
                            self.cfg_snapshot(),
                        )
                        .await
                        {
                            Ok(result)
                                if result.success
                                    && result.output.to_lowercase().trim() == "yes" =>
                            {
                                consecutive_errors = 0;
                            }
                            _ => {
                                let summary =
                                    format!("Exited due to repeated errors at step {step}: {e}");
                                let stop_reason = StopReason::Error;
                                self.tui_send(TuiEvent::Status(
                                    "Exiting due to repeated errors".into(),
                                ));
                                self.tui_send(TuiEvent::SessionFinished {
                                    stop_reason,
                                    summary_preview: summary.clone(),
                                    steps_used: step,
                                });
                                let artifacts = self.session_finish(
                                    &effective_task,
                                    &summary,
                                    stop_reason,
                                    step,
                                    session_started_at,
                                    &session_started_at_str,
                                );
                                if self.depth() == 0 {
                                    self.shutdown_tui();
                                    self.print_final_summary(
                                        stop_reason,
                                        &summary,
                                        step,
                                        artifacts.as_ref(),
                                    );
                                }
                                return Ok(());
                            }
                        }
                        tools::human::set_tui_callbacks(None, None, None, None);
                        tools::human::set_telegram_config(None, None);
                    }
                    self.history_mut().push(Turn {
                        step,
                        thought: "(error recovery)".to_string(),
                        tool: "error".to_string(),
                        args: Value::Null,
                        output: format!("ERROR: {e}"),
                        success: false,
                    });
                    if self.tui().is_some_and(|h| h.stop_requested()) {
                        let summary = format!("Stopped by user after step {step}");
                        let stop_reason = StopReason::Error;
                        self.tui_send(TuiEvent::SessionFinished {
                            stop_reason,
                            summary_preview: summary.clone(),
                            steps_used: step,
                        });
                        let artifacts = self.session_finish(
                            &effective_task,
                            &summary,
                            stop_reason,
                            step,
                            session_started_at,
                            &session_started_at_str,
                        );
                        if self.depth() == 0 {
                            self.shutdown_tui();
                            self.print_final_summary(
                                stop_reason,
                                &summary,
                                step,
                                artifacts.as_ref(),
                            );
                        }
                        return Ok(());
                    }
                }
            }
        }

        let summary = format!("Max steps ({}) reached without finish", self.max_steps());
        let stop_reason = StopReason::MaxSteps;
        self.tui_send(TuiEvent::SessionFinished {
            stop_reason,
            summary_preview: summary.clone(),
            steps_used: self.max_steps(),
        });
        let msg = format!(
            "🤖 Agent: Max steps ({}) reached without finish",
            self.max_steps()
        );
        match self.tg().send_message(&msg).await {
            Ok(resp) => {
                if !crate::tui::tui_is_active() {
                    println!("  {}", resp);
                }
            }
            Err(e) => {
                if !crate::tui::tui_is_active() {
                    println!("  Telegram error: {e}");
                }
            }
        }
        let artifacts = self.session_finish(
            &effective_task,
            &summary,
            stop_reason,
            self.max_steps(),
            session_started_at,
            &session_started_at_str,
        );
        if self.depth() == 0 {
            self.shutdown_tui();
            self.print_final_summary(stop_reason, &summary, self.max_steps(), artifacts.as_ref());
        }
        Ok(())
    }

    pub fn run_capture<'a>(
        &'a mut self,
        task: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String>> + 'a>> {
        Box::pin(async move {
            tracing::info!("[sub-agent: {}] task: {}", self.role().name(), task);
            self.session_init();
            let started_at = std::time::Instant::now();
            let started_at_str = tools::chrono_now();
            let mut consecutive_errors = 0;
            for step in 1..=self.max_steps() {
                match self.step(task, step).await {
                    Ok(StepOutcome::Continue) => {
                        consecutive_errors = 0;
                    }
                    Ok(StepOutcome::Finished {
                        summary,
                        stop_reason,
                    }) => {
                        tracing::info!(
                            "[sub-agent: {}] finished ({:?}): {}",
                            self.role().name(),
                            stop_reason,
                            summary.lines().next().unwrap_or("(no summary)")
                        );
                        self.session_finish(
                            task,
                            &summary,
                            stop_reason,
                            step,
                            started_at,
                            &started_at_str,
                        );
                        return Ok(summary);
                    }
                    Err(e) => {
                        if e.to_string().contains("Agent stuck in loop") {
                            let summary = format!(
                                "[sub-agent: {}] stopped due to no progress: {}",
                                self.role().name(),
                                e
                            );
                            self.session_finish(
                                task,
                                &summary,
                                StopReason::NoProgress,
                                step,
                                started_at,
                                &started_at_str,
                            );
                            return Ok(summary);
                        }

                        consecutive_errors += 1;
                        tracing::error!("[sub-agent] step {step} error: {e}");
                        if consecutive_errors >= 2
                            && !suppress_human_escalation_for_error(
                                self.role().name(),
                                &e.to_string(),
                            )
                        {
                            let question = format!(
                                "[sub-agent] encountered {} consecutive errors.\nLast error: {}\n\nContinue? (yes/no)",
                                consecutive_errors, e
                            );
                            let args =
                                serde_json::json!({ "prompt": question, "timeout_secs": 300 });
                            tools::human::set_telegram_config(
                                self.cfg_snapshot().telegram_token.clone(),
                                self.cfg_snapshot().telegram_chat_id.clone(),
                            );
                            match tools::dispatch_with_depth(
                                "ask_human",
                                &args,
                                self.root(),
                                self.depth(),
                                &[],
                                self.cfg_snapshot(),
                            )
                            .await
                            {
                                Ok(result)
                                    if result.success
                                        && result.output.to_lowercase().trim() == "yes" =>
                                {
                                    consecutive_errors = 0;
                                }
                                _ => {
                                    return Ok(format!(
                                        "[sub-agent: {}] exited due to repeated errors: {}",
                                        self.role().name(),
                                        e
                                    ));
                                }
                            }
                            tools::human::set_telegram_config(None, None);
                        }
                        self.history_mut().push(Turn {
                            step,
                            thought: "(error recovery)".to_string(),
                            tool: "error".to_string(),
                            args: Value::Null,
                            output: format!("ERROR: {e}"),
                            success: false,
                        });
                    }
                }
            }
            let timeout_summary = format!(
                "[sub-agent: {}] reached max_steps ({}) without finishing",
                self.role().name(),
                self.max_steps()
            );
            let _ = self
                .tg()
                .send_message(&format!(
                    "🤖 Agent: Max steps ({}) reached without finish",
                    self.max_steps()
                ))
                .await;
            self.session_finish(
                task,
                &timeout_summary,
                StopReason::MaxSteps,
                self.max_steps(),
                started_at,
                &started_at_str,
            );
            Ok(timeout_summary)
        })
    }

    fn step<'a>(
        &'a mut self,
        task: &'a str,
        step: usize,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<StepOutcome>> + 'a>> {
        Box::pin(async move {
            self.task_state_mut().set_goal(task);
            let thinking_model = self.router().resolve(
                &crate::config_struct::ModelRole::Thinking,
                self.default_model(),
            );
            let user_message = self.build_prompt(task, step);
            tracing::debug!("Prompting [{thinking_model}] (step {step})...");
            let stop = self.tui().map(|h| h.stop.clone());
            let raw = await_or_stop(
                stop,
                self.llm()
                    .with_model(&thinking_model)
                    .chat(self.system_prompt(), &user_message),
            )
            .await?;
            tracing::debug!("LLM raw:\n{raw}");
            // Estimate token counts from char lengths (chars/4 ≈ tokens for typical LLM output)
            self.tui_send(TuiEvent::Tokens {
                prompt: (user_message.len() / 4) as u32,
                output: (raw.len() / 4) as u32,
            });
            let action = match parse_action(&raw) {
                Ok(a) => {
                    self.set_consecutive_parse_failures(0);
                    a
                }
                Err(err) => {
                    return handle_parse_failure(self, err, "during initial action selection");
                }
            };
            let canonical_tool = canonical_tool_name(&action.tool)
                .unwrap_or(action.tool.as_str())
                .to_string();
            let allowed = self
                .role()
                .allowed_tools_with_groups(&self.cfg_snapshot().tool_groups);
            if !allowed.is_empty() && !allowed.contains(&canonical_tool.as_str()) {
                anyhow::bail!(
                    "Tool '{}' is not allowed for role '{}'. Allowed: {}",
                    action.tool,
                    self.role().name(),
                    allowed.join(", ")
                );
            }
            self.enforce_boss_task_source_priority(&canonical_tool, &action.args)?;
            let role = crate::config_struct::ModelRole::from_tool(&canonical_tool);
            let model = self.router().resolve(&role, self.default_model());
            let action = if model != thinking_model && action.tool != "finish" {
                tracing::debug!("Re-routing to [{model}] for role '{}'", role.label());
                let reroute_message = build_reroute_message(
                    &user_message,
                    &thinking_model,
                    &model,
                    &action.thought,
                    &action.tool,
                    &action.args,
                    &raw,
                );
                let stop = self.tui().map(|h| h.stop.clone());
                let raw2 = await_or_stop(
                    stop,
                    self.llm()
                        .with_model(&model)
                        .chat(self.system_prompt(), &reroute_message),
                )
                .await?;
                match parse_action(&raw2) {
                    Ok(a) => {
                        self.set_consecutive_parse_failures(0);
                        a
                    }
                    Err(err) => {
                        return handle_parse_failure(self, err, "during re-route");
                    }
                }
            } else {
                action
            };
            self.tui_send(TuiEvent::StepStarted {
                step,
                thought: action.thought.clone(),
                tool: canonical_tool.clone(),
                args_preview: self.tui_args_preview(&canonical_tool, &action.args),
            });
            if !crate::tui::tui_is_active() && self.depth() == 0 {
                if self.role().name() == "boss" {
                    println!(
                        "  Step {:>2}: {}",
                        step,
                        self.boss_console_step_label(&canonical_tool, &action.args)
                    );
                } else {
                    println!("  Step {:>2}: {}", step, canonical_tool);
                }
            }
            if canonical_tool == "finish" {
                let summary = action
                    .args
                    .get("summary")
                    .and_then(|v: &Value| v.as_str())
                    .unwrap_or("(no summary)")
                    .to_string();
                let stop_reason = if action
                    .args
                    .get("success")
                    .and_then(|v: &Value| v.as_bool())
                    .unwrap_or(false)
                {
                    StopReason::Success
                } else {
                    StopReason::Error
                };
                return Ok(StepOutcome::Finished {
                    summary,
                    stop_reason,
                });
            }
            if let Some(spec) = find_tool_spec(&canonical_tool) {
                match spec.status {
                    ToolStatus::Stub => {
                        if !crate::tui::tui_is_active() {
                            println!(
                                "  Note    : tool is limited; prefer fallback strategies if it does not help"
                            );
                        }
                    }
                    ToolStatus::Experimental => {
                        if !crate::tui::tui_is_active() {
                            println!("  Note    : tool is experimental; verify results carefully");
                        }
                    }
                    ToolStatus::Real => {}
                }
            }
            // Give human.rs type-erased TUI callbacks for suspend/resume/status
            if let Some(h) = self.tui() {
                let tx1 = h.tx.clone();
                let tx2 = h.tx.clone();
                let tx3 = h.tx.clone();
                let tx4 = h.tx.clone();
                tools::human::set_tui_callbacks(
                    Some(Arc::new(move || {
                        let (ack_tx, ack_rx) = std::sync::mpsc::sync_channel(0);
                        let _ = tx1.send(TuiEvent::Suspend(ack_tx));
                        let _ = ack_rx.recv();
                    })),
                    Some(Arc::new(move || {
                        let _ = tx2.send(TuiEvent::Resume);
                    })),
                    Some(Arc::new(move |msg: &str| {
                        let _ = tx3.send(TuiEvent::Status(msg.to_string()));
                    })),
                    Some(Arc::new(move |prompt: &str, timeout_secs: u64| {
                        let (resp_tx, resp_rx) = std::sync::mpsc::sync_channel(1);
                        let _ = tx4.send(TuiEvent::Prompt {
                            prompt: prompt.to_string(),
                            timeout_secs,
                            response: resp_tx,
                        });
                        resp_rx.recv().ok().flatten()
                    })),
                );
            }
            // Provide telegram credentials to human.rs for notify/ask_human
            tools::human::set_telegram_config(
                self.cfg_snapshot().telegram_token.clone(),
                self.cfg_snapshot().telegram_chat_id.clone(),
            );
            // Provide TUI sender to agents.rs so spawn_agent can forward
            // SubAgentSpawned/SubAgentFinished events to the parent TUI.
            if let Some(h) = self.tui() {
                crate::agent::spawn::set_tui_sender(Some(Arc::new(h.tx.clone())));
            }
            let stop = self.tui().map(|h| h.stop.clone());
            let result = await_or_stop(
                stop,
                tools::dispatch_with_depth(
                    &canonical_tool,
                    &action.args,
                    self.root(),
                    self.depth(),
                    &[],
                    self.cfg_snapshot(),
                ),
            )
            .await?;
            tools::human::set_tui_callbacks(None, None, None, None);
            tools::human::set_telegram_config(None, None);
            crate::agent::spawn::set_tui_sender(None);
            let annotated_output = match tool_status(&canonical_tool) {
                Some(ToolStatus::Stub) => format!("[limited tool]\n{}", result.output),
                Some(ToolStatus::Experimental) => {
                    format!("[experimental tool]\n{}", result.output)
                }
                _ => result.output.clone(),
            };
            let tui_output_preview = if canonical_tool == "spawn_agent" {
                self.spawn_agent_result_summary(&action.args, &annotated_output)
            } else {
                self.console_output_summary(&canonical_tool, &action.args, &annotated_output)
            };
            self.tui_send(TuiEvent::StepFinished {
                step,
                tool: canonical_tool.clone(),
                success: result.success,
                output_preview: tui_output_preview.clone(),
            });
            if !crate::tui::tui_is_active() && self.depth() == 0 {
                let output_label = if canonical_tool == "spawn_agent" {
                    self.spawn_agent_result_summary(&action.args, &annotated_output)
                } else {
                    self.console_output_summary(&canonical_tool, &action.args, &annotated_output)
                };
                println!(
                    "           {}{}",
                    if result.success { "OK  " } else { "ERR " },
                    output_label
                );
            }
            let turn = Turn {
                step,
                thought: action.thought.clone(),
                tool: canonical_tool.clone(),
                args: action.args.clone(),
                output: annotated_output,
                success: result.success,
            };
            self.task_state_mut().update_from_turn(&turn);
            self.history_mut().push(turn);
            if self.detect_loop() {
                anyhow::bail!("Agent stuck in loop");
            }
            Ok(StepOutcome::Continue)
        })
    }
}

#[cfg(test)]
mod tests;
