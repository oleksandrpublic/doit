use anyhow::Result;
use std::path::PathBuf;

use crate::config::{AgentConfig, ModelRole, ModelRouter, Role};
use crate::history::{History, Turn};
use crate::shell::OllamaClient;
use crate::tools::{self, LlmAction, TelegramConfig};

pub struct SweAgent {
    llm: OllamaClient,
    default_model: String,
    router: ModelRouter,
    root: PathBuf,
    history: History,
    max_steps: usize,
    max_output_chars: usize,
    system_prompt: String,
    tg: TelegramConfig,
    role: Role,
}

impl SweAgent {
    pub fn new(cfg: AgentConfig, repo: &str, max_steps: usize, role: Role) -> Result<Self> {
        let root = PathBuf::from(repo)
            .canonicalize()
            .map_err(|e| anyhow::anyhow!("Repo path '{}' not accessible: {e}", repo))?;

        let llm = OllamaClient::new(&cfg.ollama_base_url, cfg.temperature, cfg.max_tokens);

        // System prompt: role-specific (with .ai/prompts/ override) takes priority
        // over config.toml, unless config was already overridden by --system-prompt
        let system_prompt = if role != Role::Default {
            role.system_prompt(&root)
        } else {
            cfg.system_prompt
        };

        Ok(Self {
            llm,
            default_model: cfg.model,
            router: cfg.models,
            root,
            history: History::new(cfg.history_window),
            max_steps,
            max_output_chars: cfg.max_output_chars,
            system_prompt,
            tg: TelegramConfig {
                token: cfg.telegram_token,
                chat_id: cfg.telegram_chat_id,
            },
            role,
        })
    }

    fn all_models(&self) -> Vec<&str> {
        let mut seen = vec![self.default_model.as_str()];
        for opt in [
            self.router.thinking.as_deref(),
            self.router.coding.as_deref(),
            self.router.search.as_deref(),
            self.router.execution.as_deref(),
            self.router.vision.as_deref(),
        ]
        .into_iter()
        .flatten()
        {
            if !seen.contains(&opt) {
                seen.push(opt);
            }
        }
        seen
    }

    pub async fn run(&mut self, task: &str, task_image: Option<PathBuf>) -> Result<()> {
        if let Err(e) = self.llm.check_models(&self.all_models()).await {
            tracing::warn!("{e}");
        }

        println!("\n╔══════════════════════════════════════╗");
        println!("║           do_it Agent Starting       ║");
        println!("╚══════════════════════════════════════╝");
        println!("Repo : {}", self.root.display());
        println!("Role : {}", self.role.name());
        println!("Steps: max {}", self.max_steps);
        println!("Models:");
        println!("  default   : {}", self.default_model);
        if let Some(m) = &self.router.thinking  { println!("  thinking  : {m}"); }
        if let Some(m) = &self.router.coding    { println!("  coding    : {m}"); }
        if let Some(m) = &self.router.search    { println!("  search    : {m}"); }
        if let Some(m) = &self.router.execution { println!("  execution : {m}"); }
        if let Some(m) = &self.router.vision    { println!("  vision    : {m}"); }
        println!();

        // Init session: bump counter, inject last_session into history if present
        self.session_init();

        let effective_task = if let Some(img) = task_image {
            let vision_model = self.router.resolve(&ModelRole::Vision, &self.default_model);
            println!("Task : [image] {}", img.display());
            println!("       Describing with [{vision_model}]...");

            let description = self
                .llm
                .chat_with_image(
                    &vision_model,
                    &self.system_prompt,
                    "Describe this image in detail. Focus on any code, diagrams, \
                     error messages, or UI elements. This description will be used \
                     as the task for a software engineering agent.",
                    &img,
                )
                .await?;

            println!(
                "       -> {}\n",
                description.lines().next().unwrap_or("(no description)")
            );

            self.history.push(Turn {
                step: 0,
                thought: "Analysing task image".to_string(),
                tool: "read_image".to_string(),
                args: serde_json::json!({ "path": img.display().to_string() }),
                output: description.clone(),
                success: true,
            });

            description
        } else {
            println!("Task : {task}\n");
            task.to_string()
        };

        for step in 1..=self.max_steps {
            println!("--- Step {step}/{} ---", self.max_steps);

            match self.step(&effective_task, step).await {
                Ok(StepOutcome::Continue) => {}
                Ok(StepOutcome::Finished { summary, success }) => {
                    println!("\n╔══════════════════════════════════════╗");
                    println!("║            Agent Finished            ║");
                    println!("╚══════════════════════════════════════╝");
                    println!("Success: {success}");
                    println!("Summary:\n{summary}");
                    return Ok(());
                }
                Err(e) => {
                    tracing::error!("Step {step} error: {e}");
                    self.history.push(Turn {
                        step,
                        thought: "(error recovery)".to_string(),
                        tool: "error".to_string(),
                        args: serde_json::Value::Null,
                        output: format!("ERROR: {e}"),
                        success: false,
                    });
                }
            }
        }

        println!("\nMax steps ({}) reached.", self.max_steps);
        Ok(())
    }

    async fn step(&mut self, task: &str, step: usize) -> Result<StepOutcome> {
        let thinking_model = self.router.resolve(&ModelRole::Thinking, &self.default_model);
        let user_message = self.build_prompt(task, step);

        tracing::debug!("Prompting [{thinking_model}] (step {step})...");
        let raw = self.llm.chat(&thinking_model, &self.system_prompt, &user_message).await?;
        tracing::debug!("LLM raw:\n{raw}");

        let action = parse_action(&raw)?;

        // Enforce role tool allowlist
        let allowed = self.role.allowed_tools();
        if !allowed.is_empty() && !allowed.contains(&action.tool.as_str()) {
            let list = allowed.join(", ");
            anyhow::bail!(
                "Tool '{}' is not allowed for role '{}'. Allowed: {}",
                action.tool, self.role.name(), list
            );
        }

        let role = ModelRole::from_tool(&action.tool);
        let model = self.router.resolve(&role, &self.default_model);

        let action = if model != thinking_model && action.tool != "finish" {
            tracing::debug!("Re-routing to [{model}] for role '{}'", role.label());
            let raw2 = self.llm.chat(&model, &self.system_prompt, &user_message).await?;
            parse_action(&raw2)?
        } else {
            action
        };

        println!("  Model   : {} ({})", model, role.label());
        println!("  Thought : {}", action.thought);
        println!("  Tool    : {}", action.tool);
        println!("  Args    : {}", serde_json::to_string(&action.args)?);

        if action.tool == "finish" {
            let summary = action.args.get("summary")
                .and_then(|v| v.as_str())
                .unwrap_or("(no summary)")
                .to_string();
            let success = action.args.get("success")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            return Ok(StepOutcome::Finished { summary, success });
        }

        let result = tools::dispatch(
            &action.tool,
            &action.args,
            &self.root,
            self.max_output_chars,
            &self.tg,
        )
        .await?;

        println!(
            "  Output  : {}{}",
            if result.success { "OK " } else { "ERR " },
            first_line(&result.output, 120)
        );
        if result.output.len() > 100 {
            let preview: Vec<&str> = result.output.lines().take(8).collect();
            println!("  ---\n{}\n  ---", preview.join("\n"));
        }

        self.history.push(Turn {
            step,
            thought: action.thought,
            tool: action.tool,
            args: action.args,
            output: result.output,
            success: result.success,
        });

        Ok(StepOutcome::Continue)
    }

    /// Called once at session start.
    /// - Increments .ai/state/session_counter.txt
    /// - Reads .ai/state/last_session.md and injects it into history as step 0
    fn session_init(&mut self) {
        let ai_state = self.root.join(".ai/state");
        let _ = std::fs::create_dir_all(&ai_state);

        // Bump session counter
        let counter_path = ai_state.join("session_counter.txt");
        let n: u64 = std::fs::read_to_string(&counter_path)
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(0) + 1;
        let _ = std::fs::write(&counter_path, n.to_string());
        println!("Session #{n}");

        // Inject last_session into history if it exists
        let last_session_path = ai_state.join("last_session.md");
        if let Ok(content) = std::fs::read_to_string(&last_session_path) {
            if !content.trim().is_empty() {
                println!("  [memory] Restoring last_session context");
                self.history.push(Turn {
                    step: 0,
                    thought: "Restoring memory from last session".to_string(),
                    tool: "memory_read".to_string(),
                    args: serde_json::json!({ "key": "last_session" }),
                    output: content,
                    success: true,
                });
            }
        }
    }

    fn build_prompt(&self, task: &str, step: usize) -> String {
        let history = self.history.format();
        let role_hint = {
            let allowed = self.role.allowed_tools();
            if allowed.is_empty() {
                String::new()
            } else {
                format!(
                    "\n## Role: {}\nYou may ONLY use these tools: {}\n",
                    self.role.name(),
                    allowed.join(", ")
                )
            }
        };
        format!(
            "## Task\n{task}\n{role_hint}\n## History\n{history}\n\n## Instructions\n\
             You are on step {step} of {}.\n\
             Respond ONLY with a JSON object matching the format in your instructions.",
            self.max_steps
        )
    }
}

fn parse_action(raw: &str) -> Result<LlmAction> {
    let cleaned = strip_fences(raw.trim());
    let start = cleaned.find('{').ok_or_else(|| {
        anyhow::anyhow!("LLM response has no JSON:\n{}", &raw[..raw.len().min(400)])
    })?;
    let end = cleaned.rfind('}').ok_or_else(|| {
        anyhow::anyhow!("LLM response has unclosed JSON:\n{}", &raw[..raw.len().min(400)])
    })?;
    let json_str = &cleaned[start..=end];
    serde_json::from_str::<LlmAction>(json_str)
        .map_err(|e| anyhow::anyhow!("Failed to parse LLM JSON: {e}\nJSON:\n{json_str}"))
}

fn strip_fences(s: &str) -> &str {
    let s = s.strip_prefix("```json").unwrap_or(s);
    let s = s.strip_prefix("```").unwrap_or(s);
    let s = s.strip_suffix("```").unwrap_or(s);
    s.trim()
}

fn first_line(s: &str, max: usize) -> String {
    let line = s.lines().next().unwrap_or("").trim();
    if line.len() > max { format!("{}...", &line[..max]) } else { line.to_string() }
}

enum StepOutcome {
    Continue,
    Finished { summary: String, success: bool },
}
