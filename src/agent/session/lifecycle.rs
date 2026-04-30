use crate::agent::core::{StopReason, SweAgent};
use crate::agent::loops::SessionArtifacts;
use crate::config_loader::{global_boss_notes_path, global_user_profile_path};
use crate::config_struct::{AI_DIR, LOGS_DIR, STATE_DIR};
use crate::history::Turn;
use crate::redaction;
use crate::tools;

impl SweAgent {
    pub fn session_finish(
        &self,
        task: &str,
        final_summary: &str,
        stop_reason: StopReason,
        steps_used: usize,
        started_at: std::time::Instant,
        started_at_str: &str,
    ) -> Option<SessionArtifacts> {
        if self.depth() > 0 {
            return None;
        }
        let logs_dir = self.root().join(AI_DIR).join(LOGS_DIR);
        if std::fs::create_dir_all(&logs_dir).is_err() {
            return None;
        }
        let log_path = logs_dir.join(format!("session-{:03}.md", self.session_nr()));
        let trace_path = self.session_trace_path();
        let now = tools::chrono_now();
        let role = self.role().name();
        let n = self.session_nr();
        let turns: Vec<&Turn> = self.history().turns.iter().filter(|t| t.step > 0).collect();
        let total_calls = turns.len();
        let ok_calls = turns.iter().filter(|t| t.success).count();
        let err_calls = total_calls - ok_calls;
        let mut tool_counts: std::collections::HashMap<&str, (usize, usize)> = Default::default();
        for t in &turns {
            let e = tool_counts.entry(t.tool.as_str()).or_insert((0, 0));
            e.0 += 1;
            if !t.success {
                e.1 += 1;
            }
        }
        let mut tool_list: Vec<_> = tool_counts.iter().collect();
        tool_list.sort_by(|a, b| b.1.0.cmp(&a.1.0));
        let tools_section: String = tool_list
            .iter()
            .map(|(name, (calls, errs))| {
                if *errs > 0 {
                    format!("  - {name}: {calls} calls ({errs} errors)")
                } else {
                    format!("  - {name}: {calls} calls")
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        let path_sensitivity_stats = Self::trace_path_sensitivity_stats(&turns);
        let path_sensitivity_section =
            Self::render_path_sensitivity_report_section(&path_sensitivity_stats);
        let status = match stop_reason {
            StopReason::Success => "✓ success",
            StopReason::MaxSteps => "✗ stopped: max steps reached",
            StopReason::NoProgress => "✗ stopped: no progress",
            StopReason::Error => "✗ failed / incomplete",
        };
        let task_safe = redaction::redact(task);
        let summary_safe = redaction::redact(final_summary);

        // Read session_decisions.md once — used both in the log report and
        // for appending to boss_notes.md.
        let decisions_content = {
            let decisions_path = self
                .root()
                .join(AI_DIR)
                .join(STATE_DIR)
                .join("session_decisions.md");
            std::fs::read_to_string(&decisions_path)
                .ok()
                .filter(|s| !s.trim().is_empty())
        };

        let decisions_section = match &decisions_content {
            Some(content) => {
                // Truncate to last 60 lines to keep the report readable
                let lines: Vec<&str> = content.lines().collect();
                let excerpt = if lines.len() > 60 {
                    format!(
                        "<!-- {} earlier entries omitted -->\n{}",
                        lines.len() - 60,
                        lines[lines.len() - 60..].join("\n")
                    )
                } else {
                    content.clone()
                };
                format!("\n\n## Decisions\n\n{excerpt}")
            }
            None => String::new(),
        };

        let report = format!(
            "# Session #{n} — {date}\n\n**Role:** {role}  \n**Config source:** {config_source}  \n**Status:** {status}  \n**Steps used:** {steps_used}  \n**Tool calls:** {total_calls} ({ok_calls} ok, {err_calls} errors)  \n\n## Task\n\n{task_safe}\n\n## Summary\n\n{summary_safe}\n\n## Tools used\n\n{tools_section}{path_sensitivity_section}{decisions_section}\n",
            date = now,
            config_source = self.config_source()
        );
        let _ = std::fs::write(&log_path, &report);
        let trace_path = self.write_session_trace(
            trace_path,
            &task_safe,
            &summary_safe,
            stop_reason,
            steps_used,
            started_at_str,
            total_calls,
            ok_calls,
            err_calls,
            &turns,
        );
        if !crate::tui::tui_is_active() {
            println!("  [session] Report written to {}", log_path.display());
            if let Some(trace_path) = &trace_path {
                println!("  [session] Trace written to {}", trace_path.display());
            }
        }
        self.apply_task_state_persistence(stop_reason);
        self.update_last_session(&summary_safe, &task_safe, n, stop_reason, &turns);

        // Append key decisions from this session to boss_notes.md so the
        // Boss accumulates cross-session project insights.
        append_decisions_to_boss_notes(
            &task_safe,
            &summary_safe,
            stop_reason,
            n,
            &now,
            decisions_content.as_deref(),
        );

        self.run_session_cleanup(&logs_dir);
        Some(SessionArtifacts {
            log_path,
            trace_path,
            total_calls,
            ok_calls,
            err_calls,
            started_at,
            started_at_str: started_at_str.to_string(),
        })
    }

    pub(crate) fn update_last_session(
        &self,
        summary: &str,
        task: &str,
        n: u64,
        stop_reason: StopReason,
        turns: &[&Turn],
    ) {
        let state_dir = self.root().join(AI_DIR).join(STATE_DIR);
        let path = state_dir.join("last_session.md");
        let now = tools::chrono_now();
        let status_str = match stop_reason {
            StopReason::Success => "✓ success",
            StopReason::MaxSteps => "✗ max steps",
            StopReason::NoProgress => "✗ no progress",
            StopReason::Error => "✗ error",
        };
        let safety_line = Self::render_path_sensitivity_summary_line(
            &Self::trace_path_sensitivity_stats(turns),
        )
        .map(|line| format!("**Safety:** {line}\n\n"))
        .unwrap_or_default();
        let entry = format!(
            "\n## Session #{n} — {now} {status_str}\n**Task:** {task}\n\n{safety_line}{summary}\n---\n"
        );
        let existing = std::fs::read_to_string(&path).unwrap_or_default();
        let updated = format!("{existing}{entry}");
        let _ = std::fs::write(&path, &updated);
        const MAX_LINES: usize = 200;
        const KEEP_LINES: usize = 150;
        let line_count = updated.lines().count();
        if line_count > MAX_LINES {
            if !crate::tui::tui_is_active() {
                println!(
                    "  [memory] last_session.md has {line_count} lines — compressing to {KEEP_LINES}"
                );
            }
            let kept: Vec<&str> = updated
                .lines()
                .rev()
                .take(KEEP_LINES)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            let compressed = format!(
                "<!-- compressed: older entries removed, keeping last {KEEP_LINES} lines -->\n{}\n",
                kept.join("\n")
            );
            let _ = std::fs::write(&path, compressed);
        }
    }

    fn run_session_cleanup(&self, logs_dir: &std::path::Path) {
        // Cleanup old .log files (older than 30 days) from the logs directory.
        match tools::cleanup_old_logs(logs_dir, 30) {
            Ok(n) if n > 0 => {
                if !crate::tui::tui_is_active() {
                    println!("  [session] Cleaned up {n} old log file(s)");
                }
            }
            _ => {}
        }
        // If background group is enabled, clean up stale .pid files from the state directory.
        if self.cfg_snapshot().tool_groups.iter().any(|g| g == "background") {
            let state_dir = self.root().join(AI_DIR).join(STATE_DIR);
            match tools::cleanup_background_processes(&state_dir) {
                Ok(n) if n > 0 => {
                    if !crate::tui::tui_is_active() {
                        println!("  [session] Cleaned up {n} stale background process file(s)");
                    }
                }
                _ => {}
            }
        }
    }

    pub(crate) fn shutdown_tui(&mut self) {
        if let Some(mut tui) = self.take_tui() {
            tui.shutdown();
            crate::tui::set_tui_active(false);
        }
    }

    pub(crate) fn print_final_summary(
        &self,
        stop_reason: StopReason,
        summary: &str,
        steps_used: usize,
        artifacts: Option<&SessionArtifacts>,
    ) {
        for line in self.render_final_summary_lines(stop_reason, summary, steps_used, artifacts) {
            println!("{line}");
        }
    }

    pub fn session_init(&mut self) {
        let state_dir = self.root().join(AI_DIR).join(STATE_DIR);
        let counter_path = state_dir.join("session_counter.txt");
        let last_session_path = state_dir.join("last_session.md");
        let _ = std::fs::create_dir_all(&state_dir);
        self.set_resumed_from_task_state(false);
        let n = std::fs::read_to_string(&counter_path)
            .ok()
            .and_then(|s| s.trim().parse::<u64>().ok())
            .unwrap_or(0)
            + 1;
        self.set_session_nr(n);
        let _ = std::fs::write(&counter_path, n.to_string());
        if let Ok(content) = std::fs::read_to_string(&last_session_path) {
            let summary = content.lines().take(30).collect::<Vec<_>>().join("\n");
            self.history_mut().push(Turn {
                step: 0,
                thought: "Loading previous session context".to_string(),
                tool: "load_session".to_string(),
                args: serde_json::json!({}),
                output: summary,
                success: true,
            });
        }
        self.restore_task_state_from_disk();

        // Ensure user_profile.md and boss_notes.md exist for installations
        // that pre-date the Sprint 2 scaffolding. Safe to call every session:
        // only creates files that are absent, never overwrites existing ones.
        crate::config_loader::ensure_memory_files_exist();

        let boss_notes = global_boss_notes_path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .unwrap_or_default();
        let user_profile = global_user_profile_path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .unwrap_or_default();
        self.set_cached_boss_notes(boss_notes);
        self.set_cached_user_profile(user_profile);
    }
}

// ─── Boss notes update ────────────────────────────────────────────────────────
//
// Appends a brief record of the current session's decisions to `~/.do_it/boss_notes.md`
// so the Boss accumulates cross-session, cross-project insights.
//
// Written for all stop reasons (including failures) so the Boss can learn
// from unsuccessful attempts too. The entry is intentionally short:
// task (first 120 chars), summary (first 3 lines), decisions (first 10 lines).
//
// boss_notes.md is capped at BOSS_NOTES_MAX_LINES; oldest lines are dropped
// when the cap is reached, preserving the header comment block.

const BOSS_NOTES_MAX_LINES: usize = 300;
const BOSS_NOTES_KEEP_LINES: usize = 240;

fn append_decisions_to_boss_notes(
    task: &str,
    summary: &str,
    stop_reason: StopReason,
    session_nr: u64,
    now: &str,
    decisions: Option<&str>,
) {
    let path = match global_boss_notes_path() {
        Some(p) => p,
        None => return,
    };

    let status = match stop_reason {
        StopReason::Success => "✓",
        StopReason::MaxSteps => "✗ max-steps",
        StopReason::NoProgress => "✗ no-progress",
        StopReason::Error => "✗ error",
    };

    // Task: first line, capped at 120 chars
    let task_line: String = task
        .lines()
        .next()
        .unwrap_or("")
        .chars()
        .take(120)
        .collect();

    // Summary: first 3 non-empty lines
    let summary_excerpt: String = summary
        .lines()
        .filter(|l| !l.trim().is_empty())
        .take(3)
        .map(|l| format!("  {l}"))
        .collect::<Vec<_>>()
        .join("\n");

    // Decisions: first 10 lines (if any)
    let decisions_excerpt = match decisions {
        Some(d) if !d.trim().is_empty() => {
            let lines: Vec<&str> = d
                .lines()
                .filter(|l| !l.trim().is_empty())
                .take(10)
                .collect();
            format!("\n  Decisions:\n{}", lines.iter().map(|l| format!("    {l}")).collect::<Vec<_>>().join("\n"))
        }
        _ => String::new(),
    };

    let entry = format!(
        "\n## Session #{session_nr} — {now} {status}\n**Task:** {task_line}\n{summary_excerpt}{decisions_excerpt}\n---\n"
    );

    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    let updated = format!("{existing}{entry}");
    let _ = std::fs::write(&path, &updated);

    // Cap file size: keep header (lines starting with #) + most recent entries
    let line_count = updated.lines().count();
    if line_count > BOSS_NOTES_MAX_LINES {
        let lines: Vec<&str> = updated.lines().collect();

        // Find where the header comment block ends (first non-comment, non-empty line)
        let header_end = lines
            .iter()
            .position(|l| !l.trim().is_empty() && !l.starts_with('#'))
            .unwrap_or(0);

        let header: Vec<&str> = lines[..header_end].to_vec();
        let body: Vec<&str> = lines[header_end..].to_vec();

        let keep_body = body.len().saturating_sub(body.len().saturating_sub(
            BOSS_NOTES_KEEP_LINES.saturating_sub(header.len()),
        ));
        let kept_body = &body[body.len().saturating_sub(keep_body)..];

        let compressed = format!(
            "{}\n<!-- compressed: older entries removed, keeping last {} lines -->\n{}\n",
            header.join("\n"),
            BOSS_NOTES_KEEP_LINES,
            kept_body.join("\n")
        );
        let _ = std::fs::write(&path, compressed);
        tracing::debug!(
            "boss_notes.md compressed: {line_count} → {} lines",
            BOSS_NOTES_KEEP_LINES
        );
    }
}
