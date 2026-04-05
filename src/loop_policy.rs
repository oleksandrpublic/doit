pub const EXPLORATION_BUDGET: usize = 3;
pub const RECENT_PROGRESS_WINDOW: usize = 4;

pub const EXPLORATION_TOOLS: &[&str] = &[
    "read_file",
    "list_dir",
    "find_files",
    "search_in_files",
    "tree",
    "outline",
    "get_symbols",
    "open_file_region",
];

pub fn is_exploration_tool(tool: &str) -> bool {
    EXPLORATION_TOOLS.contains(&tool)
}

pub fn recent_stall_pressure(
    recent_signatures: &[String],
    recent_progress_markers: &[String],
) -> bool {
    let recent_markers = recent_progress_markers
        .iter()
        .rev()
        .take(RECENT_PROGRESS_WINDOW)
        .collect::<Vec<_>>();
    if recent_markers.len() < RECENT_PROGRESS_WINDOW {
        return false;
    }

    let exploration_steps = recent_signatures
        .iter()
        .rev()
        .take(RECENT_PROGRESS_WINDOW)
        .filter(|sig| is_exploration_signature(sig))
        .count();

    exploration_steps >= EXPLORATION_BUDGET
        && recent_markers
            .iter()
            .all(|marker| !is_meaningful_progress_marker(marker))
}

pub fn exploration_warning(
    consecutive_exploration_steps: usize,
    has_recent_stall_pressure: bool,
    has_repeated_exploration_pressure: bool,
) -> Option<&'static str> {
    if has_recent_stall_pressure {
        return Some(
            "high: at least 3 of the last 4 steps were exploration-heavy without meaningful implementation or verification progress; switch to implementation, verification, clarification, or finish with a blocker",
        );
    }

    if consecutive_exploration_steps < EXPLORATION_BUDGET {
        return None;
    }

    if has_repeated_exploration_pressure {
        Some(
            "high: 3+ consecutive exploration steps with repeated signatures detected; switch to implementation, verification, diff inspection, or finish with a blocker",
        )
    } else {
        Some(
            "medium: 3+ consecutive exploration steps detected; use the gathered context to switch to implementation, verification, diff inspection, or finish with a blocker",
        )
    }
}

pub fn exploration_budget_remaining(consecutive_exploration_steps: usize) -> usize {
    EXPLORATION_BUDGET.saturating_sub(consecutive_exploration_steps)
}

pub fn strategy_change_required(
    consecutive_exploration_steps: usize,
    has_recent_stall_pressure: bool,
) -> bool {
    consecutive_exploration_steps > EXPLORATION_BUDGET || has_recent_stall_pressure
}

pub fn is_meaningful_progress_marker(marker: &str) -> bool {
    matches!(marker, "implementation" | "verification")
}

pub fn progress_label(marker: &str) -> Option<&'static str> {
    match marker {
        "implementation" => Some("implementation"),
        "verification" => Some("verification"),
        _ => None,
    }
}

fn is_exploration_signature(signature: &str) -> bool {
    let tool = signature
        .split_once(' ')
        .map(|(tool, _)| tool)
        .unwrap_or(signature);
    is_exploration_tool(tool)
}

#[cfg(test)]
mod tests {
    use super::{
        exploration_budget_remaining, exploration_warning, recent_stall_pressure,
        strategy_change_required,
    };

    #[test]
    fn recent_stall_pressure_requires_four_recent_steps_without_meaningful_progress() {
        let recent_signatures = vec![
            "list_dir () -> src".to_string(),
            "read_file (path=a.rs) -> file".to_string(),
            "search_in_files (pattern=x) -> result".to_string(),
            "read_file (path=b.rs) -> file".to_string(),
        ];
        let recent_progress_markers = vec![
            "exploration".to_string(),
            "exploration".to_string(),
            "blocked".to_string(),
            "exploration".to_string(),
        ];

        assert!(recent_stall_pressure(
            &recent_signatures,
            &recent_progress_markers
        ));
    }

    #[test]
    fn strategy_change_required_matches_exploration_budget_contract() {
        assert_eq!(exploration_budget_remaining(2), 1);
        assert!(!strategy_change_required(3, false));
        assert!(strategy_change_required(4, false));
        assert!(strategy_change_required(1, true));
        assert!(exploration_warning(3, false, false).is_some());
    }
}
