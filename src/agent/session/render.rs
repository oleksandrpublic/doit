use crate::agent::core::{StopReason, SweAgent};
use crate::agent::loops::SessionArtifacts;
use crate::agent::session::trace::SessionTracePathSensitivityStat;
use crate::tools;

/// Marker trait / extension — rendering helpers used within this module.
/// Declared as `pub(crate)` so `lifecycle.rs` and tests can call them via
/// `SweAgentRender::final_summary_preview(...)`.
pub(crate) struct SweAgentRender;

impl SweAgentRender {
    pub(crate) fn final_summary_preview(summary: &str) -> String {
        let trimmed = summary.trim();
        if trimmed.is_empty() {
            return "(empty)".to_string();
        }
        let first_line = trimmed.lines().next().unwrap_or("").trim();
        let compact = if first_line.is_empty() { trimmed } else { first_line };
        let compact = compact.replace('\t', " ");
        if compact.len() > 220 {
            format!("{}...", &compact[..220])
        } else {
            compact
        }
    }
}

impl SweAgent {
    pub(crate) fn render_path_sensitivity_report_section(
        stats: &[SessionTracePathSensitivityStat<'static>],
    ) -> String {
        if stats.is_empty() {
            return String::new();
        }
        let body = stats
            .iter()
            .map(|stat| format!("  - {}: {} call(s)", stat.category, stat.calls))
            .collect::<Vec<_>>()
            .join("\n");
        format!("\n\n## Path sensitivity\n\n{body}")
    }

    pub(crate) fn render_path_sensitivity_summary_line(
        stats: &[SessionTracePathSensitivityStat<'static>],
    ) -> Option<String> {
        if stats.is_empty() {
            return None;
        }
        let summary = stats
            .iter()
            .take(3)
            .map(|stat| format!("{}={}", stat.category, stat.calls))
            .collect::<Vec<_>>()
            .join(", ");
        if stats.len() > 3 {
            Some(format!("{summary} (+{} more)", stats.len() - 3))
        } else {
            Some(summary)
        }
    }

    pub(crate) fn render_final_summary_lines(
        &self,
        stop_reason: StopReason,
        summary: &str,
        steps_used: usize,
        artifacts: Option<&SessionArtifacts>,
    ) -> Vec<String> {
        let mut lines = vec![
            String::new(),
            format!(
                "Result : {}",
                match stop_reason {
                    StopReason::Success => "success",
                    StopReason::MaxSteps => "stopped: max steps reached",
                    StopReason::NoProgress => "stopped: no progress",
                    StopReason::Error => "failed / incomplete",
                }
            ),
            format!("Steps  : {steps_used}/{}", self.max_steps()),
        ];

        if let Some(artifacts) = artifacts {
            let elapsed = artifacts.started_at.elapsed();
            let duration_str = if elapsed.as_secs() >= 3600 {
                format!(
                    "{}h {:02}m {:02}s",
                    elapsed.as_secs() / 3600,
                    (elapsed.as_secs() % 3600) / 60,
                    elapsed.as_secs() % 60
                )
            } else if elapsed.as_secs() >= 60 {
                format!("{}m {:02}s", elapsed.as_secs() / 60, elapsed.as_secs() % 60)
            } else {
                format!("{}s", elapsed.as_secs())
            };
            lines.push(format!("Started: {}", artifacts.started_at_str));
            lines.push(format!("Ended  : {}", tools::chrono_now()));
            lines.push(format!("Time   : {duration_str}"));
            lines.push(format!(
                "Calls  : {} total ({} ok, {} errors)",
                artifacts.total_calls, artifacts.ok_calls, artifacts.err_calls
            ));
            let sensitivity_summary =
                Self::render_path_sensitivity_summary_line(&Self::trace_path_sensitivity_stats(
                    &self
                        .history()
                        .turns
                        .iter()
                        .filter(|t| t.step > 0)
                        .collect::<Vec<_>>(),
                ));
            if let Some(sensitivity_summary) = sensitivity_summary {
                lines.push(format!("Safety : {sensitivity_summary}"));
            }
            lines.push(format!("Config : {}", self.config_source()));
            lines.push(format!("Report : {}", artifacts.log_path.display()));
            if let Some(trace_path) = &artifacts.trace_path {
                lines.push(format!("Trace  : {}", trace_path.display()));
            }
        }
        lines.push(format!(
            "Summary: {}",
            SweAgentRender::final_summary_preview(summary)
        ));
        lines
    }
}
