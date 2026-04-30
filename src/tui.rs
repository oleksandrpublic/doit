//! Terminal UI for do_it — three-panel live view.
//!
//! Activated only when stdout is a TTY and depth == 0.
//! Sub-agents bypass TUI entirely and write to the step log via events.
//!
//! Layout:
//!   ┌─────────────────────────────────────────────┐
//!   │  do_it  vX.Y.Z   task: "..."   role: boss   │  <- header
//!   ├───────────────────┬─────────────────────────┤
//!   │  PROGRESS         │  STEP LOG               │
//!   │  (stats)          │  (scrollable)           │
//!   ├───────────────────┴─────────────────────────┤
//!   │  STATUS  current action                     │  <- footer
//!   └─────────────────────────────────────────────┘
//!
//! Keys: q / Ctrl-C = graceful stop, ↑↓ = scroll log

use std::io::Write;
use std::io::{self, IsTerminal};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::{
    cursor::Show,
    event::{self, DisableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    buffer::Buffer,
    layout::{Alignment, Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Gauge, List, ListItem, Paragraph, Wrap},
    Terminal,
};
use tokio::sync::mpsc;

use crate::agent::core::StopReason;
use crate::text::truncate_chars;

// ─── Global TUI active flag ───────────────────────────────────────────────────
// Set to true when top-level agent activates TUI.
// Sub-agents and step() check this to suppress stdout output.

static TUI_ACTIVE: AtomicBool = AtomicBool::new(false);

pub fn set_tui_active(active: bool) {
    TUI_ACTIVE.store(active, Ordering::Relaxed);
}

pub fn tui_is_active() -> bool {
    TUI_ACTIVE.load(Ordering::Relaxed)
}

// ─── Events sent from the agent loop to the TUI thread ───────────────────────

#[derive(Debug)]
pub enum TuiEvent {
    /// Agent started a new session
    SessionStarted {
        task: String,
        role: String,
        max_steps: usize,
        repo: String,
    },
    /// About to call a tool on this step
    StepStarted {
        step: usize,
        thought: String,
        tool: String,
        args_preview: String,
    },
    /// Tool returned a result
    StepFinished {
        step: usize,
        tool: String,
        success: bool,
        output_preview: String,
    },
    /// Token counts for the current LLM call
    Tokens { prompt: u32, output: u32 },
    /// A sub-agent was spawned
    SubAgentSpawned {
        role: String,
        task_preview: String,
        depth: usize,
    },
    /// Sub-agent finished
    SubAgentFinished { role: String, depth: usize },
    /// Short status line update (footer)
    Status(String),
    /// Session ended
    SessionFinished {
        stop_reason: StopReason,
        summary_preview: String,
        steps_used: usize,
    },
    /// Prompt the user for a short response inside the TUI.
    Prompt {
        prompt: String,
        timeout_secs: u64,
        response: tokio::sync::oneshot::Sender<Option<String>>,
    },
    /// Cancel an in-progress Prompt (e.g. because Telegram answered first).
    /// The active prompt_state is dismissed by sending None into its response
    /// channel; no stop flag is set and the TUI continues running normally.
    CancelPrompt,
    /// Suspend TUI temporarily (e.g. for stdin input). TUI exits alternate screen.
    /// The oneshot sender is signalled once TUI has fully suspended.
    Suspend(tokio::sync::oneshot::Sender<()>),
    /// Resume TUI after suspension.
    Resume,
    /// Request graceful shutdown
    Quit,
}

// ─── Handle returned to the agent loop ───────────────────────────────────────

pub struct TuiHandle {
    pub tx: mpsc::UnboundedSender<TuiEvent>,
    pub stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl TuiHandle {
    pub fn send(&self, ev: TuiEvent) {
        let _ = self.tx.send(ev);
    }

    /// True when the user pressed q / Ctrl-C in the TUI
    pub fn stop_requested(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }

    /// Suspend the TUI so stdin becomes readable. Blocks until TUI has exited
    /// alternate screen. Must be paired with resume().
    pub fn suspend(&self) {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let _ = self.tx.send(TuiEvent::Suspend(tx));
        // Block until TUI confirms it has left alternate screen.
        // This is called from a sync context (spawn_blocking inside human.rs),
        // so we drive the oneshot to completion via the current tokio Handle.
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            let _ = handle.block_on(rx);
        }
    }

    /// Resume the TUI after suspend().
    pub fn resume(&self) {
        let _ = self.tx.send(TuiEvent::Resume);
    }

    /// Request shutdown and wait for the TUI thread to restore the terminal.
    pub fn shutdown(&mut self) {
        let _ = self.tx.send(TuiEvent::Quit);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

// ─── Internal state ──────────────────────────────────────────────────────────

struct TuiState {
    task: String,
    role: String,
    repo: String,
    max_steps: usize,
    current_step: usize,
    status: String,
    last_decision: String,
    log: Vec<LogEntry>,
    log_scroll: usize,
    tokens_prompt_total: u32,
    tokens_output_total: u32,
    tokens_prompt_step: u32,
    tokens_output_step: u32,
    subagents_stored: u32,
    subagents_empty: u32,
    subagents_error: u32,
    started_at: Instant,
    finished: Option<(StopReason, String)>,
    active_sub_agents: Vec<String>,
    prompt_state: Option<PromptState>,
}

#[derive(Clone)]
struct LogEntry {
    step: usize,
    success: Option<bool>, // None = in progress
    tool: String,
    text: String,
}

struct PromptState {
    prompt: String,
    input: String,
    response: tokio::sync::oneshot::Sender<Option<String>>,
    deadline: Option<Instant>,
}

impl TuiState {
    fn new() -> Self {
        Self {
            task: String::new(),
            role: String::new(),
            repo: String::new(),
            max_steps: 30,
            current_step: 0,
            status: "Initialising...".into(),
            last_decision: String::new(),
            log: Vec::new(),
            log_scroll: 0,
            tokens_prompt_total: 0,
            tokens_output_total: 0,
            tokens_prompt_step: 0,
            tokens_output_step: 0,
            subagents_stored: 0,
            subagents_empty: 0,
            subagents_error: 0,
            started_at: Instant::now(),
            finished: None,
            active_sub_agents: Vec::new(),
            prompt_state: None,
        }
    }

    fn apply(&mut self, ev: TuiEvent) {
        match ev {
            TuiEvent::SessionStarted {
                task,
                role,
                max_steps,
                repo,
            } => {
                self.task = truncate_chars(&task, 60);
                self.role = role;
                self.max_steps = max_steps;
                self.repo = repo;
                self.started_at = Instant::now();
                self.status = "Starting…".into();
            }
            TuiEvent::StepStarted {
                step,
                thought,
                tool,
                args_preview,
            } => {
                self.current_step = step;
                self.tokens_prompt_step = 0;
                self.tokens_output_step = 0;
                self.status = format!("step {step}  {tool}");
                if !thought.is_empty() {
                    self.last_decision = truncate_chars(&thought, 28);
                }
                self.log.push(LogEntry {
                    step,
                    success: None,
                    tool: tool.clone(),
                    text: if args_preview.is_empty() {
                        format!("step {step:3}  ·  {tool}")
                    } else {
                        format!("step {step:3}  ·  {tool}  {args_preview}")
                    },
                });
                // Auto-scroll to bottom
                self.log_scroll = self.log.len().saturating_sub(1);
            }
            TuiEvent::StepFinished {
                step,
                tool,
                success,
                output_preview,
            } => {
                let icon = if success { "✓" } else { "✗" };
                if tool == "spawn_agent" {
                    let preview = output_preview.to_ascii_lowercase();
                    if !success || preview.contains(" err") || preview.contains("error") {
                        self.subagents_error += 1;
                    } else if preview.contains(" empty") {
                        self.subagents_empty += 1;
                    } else if preview.contains(" stored") {
                        self.subagents_stored += 1;
                    }
                }
                // Update the matching in-progress entry
                if let Some(entry) = self
                    .log
                    .iter_mut()
                    .rev()
                    .find(|e| e.step == step && e.tool == tool)
                {
                    entry.success = Some(success);
                    entry.text = format!(
                        "step {step:3}  {icon}  {tool}  {}",
                        truncate_chars(&output_preview, 120)
                    );
                }
                self.status = format!("step {step}  {icon}  {tool}");
                self.log_scroll = self.log.len().saturating_sub(1);
            }
            TuiEvent::Tokens { prompt, output } => {
                self.tokens_prompt_step = prompt;
                self.tokens_output_step = output;
                self.tokens_prompt_total += prompt;
                self.tokens_output_total += output;
            }
            TuiEvent::SubAgentSpawned {
                role,
                task_preview,
                depth,
            } => {
                let label = format!("{role}(d{depth})");
                self.active_sub_agents.push(label.clone());
                self.log.push(LogEntry {
                    step: self.current_step,
                    success: None,
                    tool: "spawn_agent".into(),
                    text: format!("  >> sub-agent {label}: {task_preview}"),
                });
                self.log_scroll = self.log.len().saturating_sub(1);
            }
            TuiEvent::SubAgentFinished { role, depth } => {
                let label = format!("{role}(d{depth})");
                self.active_sub_agents.retain(|s| s != &label);
                self.log.push(LogEntry {
                    step: self.current_step,
                    success: Some(true),
                    tool: "spawn_agent".into(),
                    text: format!("  << sub-agent {label} done"),
                });
                self.log_scroll = self.log.len().saturating_sub(1);
            }
            TuiEvent::Status(s) => {
                self.status = s;
            }
            TuiEvent::SessionFinished {
                stop_reason,
                summary_preview,
                steps_used: _,
            } => {
                self.finished = Some((stop_reason, summary_preview));
                self.status = match stop_reason {
                    StopReason::Success => "✓ Done".into(),
                    StopReason::MaxSteps => "✗ Stopped: max steps reached".into(),
                    StopReason::NoProgress => "✗ Stopped: no progress".into(),
                    StopReason::Error => "✗ Failed / incomplete".into(),
                };
            }
            TuiEvent::Prompt {
                prompt,
                timeout_secs,
                response,
            } => {
                self.status = "Waiting for input".into();
                self.prompt_state = Some(PromptState {
                    prompt,
                    input: String::new(),
                    response,
                    deadline: if timeout_secs == 0 {
                        None
                    } else {
                        Some(Instant::now() + Duration::from_secs(timeout_secs))
                    },
                });
            }
            TuiEvent::CancelPrompt => {
                // Dismiss active prompt without setting stop flag.
                // Used when Telegram (or another channel) answered first.
                if let Some(prompt) = self.prompt_state.take() {
                    let _ = prompt.response.send(None);
                    self.status = "Input received via Telegram".into();
                }
            }
            TuiEvent::Quit => {}
            TuiEvent::Suspend(_) | TuiEvent::Resume => {}
        }
    }

    fn elapsed_str(&self) -> String {
        let s = self.started_at.elapsed().as_secs();
        format!("{:02}:{:02}:{:02}", s / 3600, (s % 3600) / 60, s % 60)
    }

    fn eta_str(&self) -> String {
        if self.current_step == 0 {
            return "  --:--".into();
        }
        let elapsed = self.started_at.elapsed().as_secs_f64();
        let secs_per_step = elapsed / self.current_step as f64;
        let remaining_steps = self.max_steps.saturating_sub(self.current_step);
        let eta_secs = (secs_per_step * remaining_steps as f64) as u64;
        format!(
            "~{:02}:{:02}:{:02}",
            eta_secs / 3600,
            (eta_secs % 3600) / 60,
            eta_secs % 60
        )
    }
}

// ─── Public entry point ──────────────────────────────────────────────────────

/// Returns None when stdout is not a TTY (CI / pipe — use plain output).
#[track_caller]
pub fn start() -> Option<TuiHandle> {
    if !io::stdout().is_terminal() {
        return None;
    }

    let (tx, rx) = mpsc::unbounded_channel::<TuiEvent>();
    let stop = Arc::new(AtomicBool::new(false));
    let stop_clone = stop.clone();

    let thread = std::thread::spawn(move || {
        // Install a panic hook that restores the terminal before printing
        // the panic message. Without this, a panic leaves the terminal in
        // raw/alternate-screen mode and requires `reset` to recover.
        let default_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            // Best-effort terminal restore — ignore errors
            let _ = disable_raw_mode();
            let mut stdout = io::stdout();
            let _ = execute!(stdout, LeaveAlternateScreen, DisableMouseCapture, Show,);
            default_hook(info);
        }));

        if let Err(e) = run_tui(rx, stop_clone) {
            // TUI crashed — restore terminal then print error
            let _ = disable_raw_mode();
            let mut stdout = io::stdout();
            let _ = execute!(stdout, LeaveAlternateScreen, DisableMouseCapture, Show,);
            eprintln!("[TUI error] {e}");
        }
    });

    Some(TuiHandle {
        tx,
        stop,
        thread: Some(thread),
    })
}

// ─── TUI render loop (runs in its own thread) ─────────────────────────────────

fn run_tui(mut rx: mpsc::UnboundedReceiver<TuiEvent>, stop: Arc<AtomicBool>) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut state = TuiState::new();
    let mut running = true;

    while running {
        // Drain all pending events (non-blocking)
        loop {
            match rx.try_recv() {
                Ok(TuiEvent::Quit) => {
                    running = false;
                    break;
                }
                Ok(TuiEvent::Suspend(ack)) => {
                    // Leave alternate screen so stdin works normally
                    restore_terminal(&mut terminal);
                    // Signal the caller that we have suspended
                    let _ = ack.send(());
                    // Wait for Resume event (blocking)
                    loop {
                        match rx.blocking_recv() {
                            Some(TuiEvent::Resume) => break,
                            Some(TuiEvent::Quit) => {
                                running = false;
                                break;
                            }
                            None => {
                                running = false;
                                break;
                            }
                            _ => {}
                        }
                    }
                    if running {
                        // Re-enter alternate screen
                        let _ = enable_raw_mode();
                        let _ = execute!(terminal.backend_mut(), EnterAlternateScreen);
                        // Force a redraw immediately
                        let _ = terminal.draw(|f| render(f, &state));
                    }
                    break;
                }
                Ok(TuiEvent::Resume) => {}
                Ok(ev) => {
                    state.apply(ev);
                }
                Err(_) => break,
            }
        }

        if let Some(prompt) = state.prompt_state.as_ref() {
            if prompt
                .deadline
                .is_some_and(|deadline| Instant::now() >= deadline)
            {
                if let Some(prompt) = state.prompt_state.take() {
                    let _ = prompt.response.send(None);
                    state.status = "ask_human timed out".into();
                }
            }
        }

        terminal.draw(|f| render(f, &state))?;

        // Check keyboard (16ms poll = ~60fps)
        if event::poll(Duration::from_millis(16))? {
            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press {
                    continue;
                }
                if state.prompt_state.is_some() {
                    match key.code {
                        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            if let Some(ps) = state.prompt_state.take() {
                                let _ = ps.response.send(None);
                            }
                            stop.store(true, Ordering::Relaxed);
                            state.status = "Stop requested".into();
                            running = false;
                        }
                        KeyCode::Char('q') if key.modifiers.is_empty() => {
                            if let Some(ps) = state.prompt_state.take() {
                                let _ = ps.response.send(None);
                            }
                            stop.store(true, Ordering::Relaxed);
                            state.status = "Stop requested".into();
                            running = false;
                        }
                        KeyCode::Enter => {
                            if let Some(ps) = state.prompt_state.take() {
                                let value = ps.input.trim().to_string();
                                let response = if value.is_empty() {
                                    Some("(no input provided)".to_string())
                                } else {
                                    Some(value)
                                };
                                let _ = ps.response.send(response);
                            }
                            state.status = "Input submitted".into();
                        }
                        KeyCode::Char(c) => {
                            if !key.modifiers.contains(KeyModifiers::CONTROL) {
                                if let Some(ps) = state.prompt_state.as_mut() {
                                    ps.input.push(c);
                                }
                            }
                        }
                        KeyCode::Backspace => {
                            if let Some(ps) = state.prompt_state.as_mut() {
                                ps.input.pop();
                            }
                        }
                        KeyCode::Esc => {
                            if let Some(ps) = state.prompt_state.take() {
                                let _ = ps.response.send(None);
                            }
                            state.status = "Input cancelled".into();
                        }
                        _ => {}
                    }
                    continue;
                }
                match key.code {
                    KeyCode::Char('q') => {
                        stop.store(true, Ordering::Relaxed);
                        state.status = "Stop requested — finishing current step".into();
                        running = false;
                    }
                    KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        stop.store(true, Ordering::Relaxed);
                        state.status = "Stop requested — finishing current step".into();
                        running = false;
                    }
                    KeyCode::Up => {
                        state.log_scroll = state.log_scroll.saturating_sub(1);
                    }
                    KeyCode::Down => {
                        state.log_scroll =
                            (state.log_scroll + 1).min(state.log.len().saturating_sub(1));
                    }
                    _ => {}
                }
            }
        }

        if !running {
            break;
        }
        if state.finished.is_some() {
            std::thread::sleep(Duration::from_millis(1500));
            break;
        }
    }

    restore_terminal(&mut terminal);
    Ok(())
}

fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) {
    let _ = terminal.show_cursor();
    let _ = disable_raw_mode();
    let _ = terminal.flush();
    let mut stdout = io::stdout();
    let _ = execute!(stdout, LeaveAlternateScreen, DisableMouseCapture, Show);
    let _ = stdout.flush();
}

// ─── Rendering ────────────────────────────────────────────────────────────────

fn render(f: &mut ratatui::Frame, state: &TuiState) {
    let area = f.area();

    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .split(area);

    let header_text = format!(
        " do_it   task: {}   role: {}   {}",
        state.task,
        state.role,
        state.elapsed_str()
    );
    let header = Paragraph::new(header_text).style(
        Style::default()
            .bg(Color::Blue)
            .fg(Color::White)
            .add_modifier(Modifier::BOLD),
    );
    f.render_widget(header, outer[0]);

    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(32), Constraint::Min(0)])
        .split(outer[1]);

    let progress_ratio = if state.max_steps > 0 {
        state.current_step as f64 / state.max_steps as f64
    } else {
        0.0
    };

    let stats_lines = vec![
        Line::from(vec![
            Span::styled("Session step  ", Style::default().fg(Color::Gray)),
            Span::styled(
                format!("{}/{}", state.current_step, state.max_steps),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("Role          ", Style::default().fg(Color::Gray)),
            Span::styled(&state.role, Style::default().fg(Color::LightYellow)),
        ]),
        Line::from(vec![
            Span::styled("Elapsed       ", Style::default().fg(Color::Gray)),
            Span::styled(state.elapsed_str(), Style::default().fg(Color::White)),
        ]),
        Line::from(vec![
            Span::styled("ETA           ", Style::default().fg(Color::Gray)),
            Span::styled(state.eta_str(), Style::default().fg(Color::White)),
        ]),
        Line::from(""),
        Line::from(vec![Span::styled(
            "Tokens (step) ",
            Style::default().fg(Color::Gray),
        )]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                format!(
                    "in {:>6}  out {:>5}",
                    state.tokens_prompt_step, state.tokens_output_step
                ),
                Style::default().fg(Color::White),
            ),
        ]),
        Line::from(vec![Span::styled(
            "Tokens (total)",
            Style::default().fg(Color::Gray),
        )]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                format!(
                    "in {:>6}  out {:>5}",
                    state.tokens_prompt_total, state.tokens_output_total
                ),
                Style::default().fg(Color::Cyan),
            ),
        ]),
        Line::from(""),
        Line::from(vec![Span::styled(
            "Last decision ",
            Style::default().fg(Color::Gray),
        )]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                if state.last_decision.is_empty() {
                    "—".into()
                } else {
                    state.last_decision.clone()
                },
                Style::default().fg(Color::White),
            ),
        ]),
        Line::from(""),
        Line::from(vec![Span::styled(
            "Sub-agents    ",
            Style::default().fg(Color::Gray),
        )]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                format!("active {:>2}", state.active_sub_agents.len()),
                Style::default().fg(Color::LightYellow),
            ),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled("stored ", Style::default().fg(Color::Gray)),
            Span::styled(
                format!("{:>2}", state.subagents_stored),
                Style::default()
                    .fg(Color::LightGreen)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
            Span::styled("empty ", Style::default().fg(Color::Gray)),
            Span::styled(
                format!("{:>2}", state.subagents_empty),
                if state.subagents_empty > 0 {
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::White)
                },
            ),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled("error  ", Style::default().fg(Color::Gray)),
            Span::styled(
                format!("{:>2}", state.subagents_error),
                if state.subagents_error > 0 {
                    Style::default()
                        .fg(Color::LightRed)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::White)
                },
            ),
        ]),
    ];

    let stats_block = Block::default()
        .title(" PROGRESS ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Gray));

    let stats_inner = stats_block.inner(body[0]);
    f.render_widget(stats_block, body[0]);

    let stats_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(1)])
        .split(stats_inner);

    let stats_para = Paragraph::new(stats_lines);
    f.render_widget(stats_para, stats_layout[0]);

    let gauge = Gauge::default()
        .gauge_style(Style::default().fg(Color::Green).bg(Color::DarkGray))
        .ratio(progress_ratio.clamp(0.0, 1.0));
    f.render_widget(gauge, stats_layout[1]);

    let log_height = body[1].height.saturating_sub(2) as usize;
    let total = state.log.len();
    let scroll = state.log_scroll.min(total.saturating_sub(1));
    let start = if total > log_height {
        scroll.min(total - log_height)
    } else {
        0
    };
    let visible: Vec<ListItem> = state.log[start..]
        .iter()
        .take(log_height)
        .map(|entry| {
            let style = match entry.success {
                None => Style::default().fg(Color::LightYellow),
                Some(true) => Style::default().fg(Color::LightGreen),
                Some(false) => Style::default().fg(Color::LightRed),
            };
            ListItem::new(Line::from(Span::styled(entry.text.clone(), style)))
        })
        .collect();

    let scroll_info = if total > log_height {
        format!(" STEP LOG ({}/{}) ↑↓ ", scroll + 1, total)
    } else {
        " STEP LOG ".into()
    };

    let log_list = List::new(visible).block(
        Block::default()
            .title(scroll_info)
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Gray)),
    );
    f.render_widget(log_list, body[1]);

    let footer_style = if state
        .finished
        .as_ref()
        .map(|(reason, _)| reason.is_success())
        .unwrap_or(true)
    {
        Style::default().bg(Color::DarkGray).fg(Color::White)
    } else {
        Style::default().bg(Color::Red).fg(Color::White)
    };
    let max_status_chars = area.width.saturating_sub(28) as usize;
    let footer_text = format!(
        " {}  │  q=quit  ↑↓=scroll",
        truncate_chars(&state.status, max_status_chars)
    );
    let footer = Paragraph::new(footer_text)
        .style(footer_style)
        .alignment(Alignment::Left)
        .wrap(Wrap { trim: true });
    f.render_widget(footer, outer[2]);

    if let Some(prompt) = state.prompt_state.as_ref() {
        render_prompt(f, area, prompt);
    }
}

fn render_prompt(f: &mut ratatui::Frame, area: ratatui::layout::Rect, prompt: &PromptState) {
    let popup_inner_width = (area.width as usize * 70 / 100).saturating_sub(2).max(20);
    let prompt_chars = prompt.prompt.chars().count();
    let prompt_lines = prompt_chars.div_ceil(popup_inner_width).max(1);
    let popup_height = (prompt_lines + 5).min(area.height.saturating_sub(2) as usize);
    let popup = centered_rect(70, popup_height as u16, area);
    let block = Block::default()
        .title(" ASK HUMAN ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Yellow))
        .style(Style::default().bg(Color::Black));

    let inner = block.inner(popup);
    f.render_widget(ClearWidget, popup);
    f.render_widget(block, popup);

    let lines = vec![
        Line::from(prompt.prompt.clone()),
        Line::from(""),
        Line::from(vec![
            Span::styled("> ", Style::default().fg(Color::Yellow)),
            Span::raw(prompt.input.clone()),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "Enter=submit  Esc=cancel",
            Style::default().fg(Color::Gray),
        )),
    ];
    let paragraph = Paragraph::new(lines).wrap(Wrap { trim: true });
    f.render_widget(paragraph, inner);

    let prompt_width = inner.width.max(1) as usize;
    let prompt_chars: usize = prompt.prompt.chars().count();
    let prompt_rows = prompt_chars.div_ceil(prompt_width);
    let input_row = inner.y + prompt_rows as u16 + 1;
    let input_col = inner.x + 2 + prompt.input.chars().count() as u16;
    let input_col = input_col.min(inner.x + inner.width.saturating_sub(1));
    f.set_cursor_position((input_col, input_row));
}

fn centered_rect(
    percent_x: u16,
    height: u16,
    area: ratatui::layout::Rect,
) -> ratatui::layout::Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Fill(1),
            Constraint::Length(height),
            Constraint::Fill(1),
        ])
        .split(area);
    let horizontal = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(vertical[1]);
    horizontal[1]
}

struct ClearWidget;

impl ratatui::widgets::Widget for ClearWidget {
    fn render(self, area: ratatui::layout::Rect, buf: &mut Buffer) {
        for y in area.top()..area.bottom() {
            for x in area.left()..area.right() {
                buf[(x, y)].reset();
                buf[(x, y)].set_bg(Color::Black);
            }
        }
    }
}
