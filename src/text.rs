//! Shared text utility functions used across multiple modules.
//!
//! Lives here (not in `agent::tools` or `task_state`) to avoid circular imports:
//! both `task_state` and `agent` need `first_line`, but `task_state` is a
//! dependency of `agent`, so neither can own the function.

/// Return the first line of `s`, truncated to at most `max` Unicode scalar
/// values. Appends '…' when truncated.
pub fn first_line(s: &str, max: usize) -> String {
    let line = s.lines().next().unwrap_or("").trim();
    let mut chars = line.chars();
    let collected: String = chars.by_ref().take(max).collect();
    if chars.next().is_some() {
        format!("{collected}…")
    } else {
        collected
    }
}

/// Truncate a string to at most `max_chars` Unicode scalar values,
/// appending '…' if truncated. Safe with any UTF-8 input.
pub fn truncate_chars(s: &str, max_chars: usize) -> String {
    let mut chars = s.chars();
    let collected: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{collected}…")
    } else {
        collected
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_line_empty_string() {
        assert_eq!(first_line("", 10), "");
    }

    #[test]
    fn first_line_single_line_within_limit() {
        assert_eq!(first_line("hello", 10), "hello");
    }

    #[test]
    fn first_line_truncates_long_line() {
        assert_eq!(first_line("abcdef", 4), "abcd…");
    }

    #[test]
    fn first_line_returns_only_first_line() {
        assert_eq!(first_line("line one\nline two", 100), "line one");
    }

    #[test]
    fn first_line_trims_leading_whitespace() {
        assert_eq!(first_line("  hello", 10), "hello");
    }

    #[test]
    fn truncate_chars_within_limit() {
        assert_eq!(truncate_chars("hello", 10), "hello");
    }

    #[test]
    fn truncate_chars_exactly_at_limit() {
        assert_eq!(truncate_chars("hello", 5), "hello");
    }

    #[test]
    fn truncate_chars_over_limit() {
        assert_eq!(truncate_chars("abcdef", 4), "abcd…");
    }

    #[test]
    fn truncate_chars_unicode() {
        // Each emoji is one scalar value
        let s = "😀😁😂😃😄";
        assert_eq!(truncate_chars(s, 3), "😀😁😂…");
    }
}
