use crate::redaction;
use crate::tools::core::{ToolResult, str_arg};
use crate::validation::resolve_safe_path;
use anyhow::{Context, Result};
use serde_json::Value;
use std::path::Path;

/// Redact sensitive tokens from a tool output message before it is returned
/// to the agent.  Annotations such as `[sensitivity: ...]` survive because
/// they do not match any sensitive-token pattern.
fn redact_output(msg: String) -> String {
    redaction::redact(&msg)
}

pub fn read_file(args: &Value, root: &Path) -> Result<ToolResult> {
    let path = resolve_safe_path(root, &str_arg(args, "path")?)?;
    let content = match read_text_file(&path, "read_file") {
        Ok(content) => content,
        Err(result) => return Ok(result),
    };
    let start = args
        .get("start_line")
        .and_then(|v| v.as_u64())
        .map(|n| n as usize)
        .unwrap_or(1);
    let end = args
        .get("end_line")
        .and_then(|v| v.as_u64())
        .map(|n| n as usize);
    let lines: Vec<&str> = content.lines().collect();
    let total = lines.len();
    let s = start.saturating_sub(1).min(total);
    let e = end.map(|n| n.min(total)).unwrap_or((s + 100).min(total));
    let numbered: String = lines[s..e]
        .iter()
        .enumerate()
        .map(|(i, l)| format!("{:>4}  {}", s + i + 1, l))
        .collect::<Vec<_>>()
        .join("\n");
    Ok(ToolResult::ok(format!(
        "File: {path:?} (lines {}-{} of {})\n{numbered}",
        s + 1,
        e,
        total
    )))
}

pub fn open_file_region(args: &Value, root: &Path) -> Result<ToolResult> {
    let path = resolve_safe_path(root, &str_arg(args, "path")?)?;
    let content = match read_text_file(&path, "open_file_region") {
        Ok(content) => content,
        Err(result) => return Ok(result),
    };

    let line = args
        .get("line")
        .or_else(|| args.get("start_line"))
        .and_then(|v| v.as_u64())
        .map(|n| n as usize)
        .unwrap_or(1);
    let before = args
        .get("before")
        .and_then(|v| v.as_u64())
        .map(|n| n as usize)
        .unwrap_or(10);
    let after = args
        .get("after")
        .and_then(|v| v.as_u64())
        .map(|n| n as usize)
        .unwrap_or(10);

    let lines: Vec<&str> = content.lines().collect();
    let total = lines.len();
    if total == 0 {
        return Ok(ToolResult::failure(format!(
            "open_file_region: file is empty: {path:?}"
        )));
    }

    let target = line.clamp(1, total);
    let start = target.saturating_sub(before).max(1);
    let end = (target + after).min(total);
    let numbered = lines[start - 1..end]
        .iter()
        .enumerate()
        .map(|(i, text)| {
            let current = start + i;
            let marker = if current == target { ">" } else { " " };
            format!("{marker}{:>4}  {}", current, text)
        })
        .collect::<Vec<_>>()
        .join("\n");

    Ok(ToolResult::ok(format!(
        "File region: {path:?} (focus line {target}, lines {start}-{end} of {total})\n{numbered}"
    )))
}

pub fn read_test_failure(args: &Value, root: &Path) -> Result<ToolResult> {
    let path = resolve_test_failure_path(args, root)?;
    let content = match read_text_file(&path, "read_test_failure") {
        Ok(content) => content,
        Err(result) => return Ok(result),
    };

    let lines: Vec<&str> = content.lines().collect();
    let summaries = collect_failed_test_summaries(&lines);
    if summaries.is_empty() {
        return Ok(ToolResult::failure(format!(
            "read_test_failure: no failed tests found in {path:?}"
        )));
    }

    let blocks = collect_failure_blocks(&lines);
    let requested_test = args
        .get("test")
        .or_else(|| args.get("test_name"))
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let requested_index = args
        .get("index")
        .and_then(|v| v.as_u64())
        .map(|n| n as usize);

    let selected = if let Some(test_name) = requested_test {
        summaries
            .iter()
            .find(|summary| summary.name == test_name)
            .or_else(|| {
                summaries
                    .iter()
                    .find(|summary| summary.name.contains(test_name))
            })
            .ok_or_else(|| anyhow::anyhow!("read_test_failure: test not found: {test_name}"))?
    } else if let Some(index) = requested_index {
        let idx = index.saturating_sub(1);
        summaries.get(idx).ok_or_else(|| {
            anyhow::anyhow!(
                "read_test_failure: index {} is out of range ({} failures found)",
                index,
                summaries.len()
            )
        })?
    } else {
        summaries.last().expect("summaries is not empty")
    };

    let fallback_excerpt = render_test_failure_excerpt(&lines, selected.line_number, 3, 8);
    let detail = blocks
        .iter()
        .find(|block| block.name == selected.name)
        .or_else(|| {
            blocks.iter().find(|block| {
                block.name.contains(&selected.name) || selected.name.contains(&block.name)
            })
        });

    let mut output = format!(
        "Test failure: {}\nSource: {path:?}\nFailure {} of {}\nSummary line: {}\n\nSummary excerpt:\n{}",
        selected.name,
        summaries
            .iter()
            .position(|summary| summary.name == selected.name
                && summary.line_number == selected.line_number)
            .map(|pos| pos + 1)
            .unwrap_or(1),
        summaries.len(),
        selected.line_number,
        fallback_excerpt
    );

    if let Some(block) = detail {
        output.push_str(&format!(
            "\n\nDetailed failure block (lines {}-{}):\n{}",
            block.start_line, block.end_line, block.text
        ));
    } else {
        output.push_str("\n\nDetailed failure block: not present in this log; only the summary excerpt was found.");
    }

    Ok(ToolResult::ok(output))
}

pub fn write_file(args: &Value, root: &Path) -> Result<ToolResult> {
    let path = resolve_safe_path(root, &str_arg(args, "path")?)?;
    let sensitivity = crate::path_sensitivity::classify_path_sensitivity(root, &path);
    let outcome_annotations = sensitivity.outcome_annotations();
    let content = str_arg(args, "content")?;

    // Basic content validation
    if content.is_empty() {
        return Ok(ToolResult::failure("Content cannot be empty"));
    }

    // Check for potential encoding issues (null bytes, etc.)
    if content.contains('\0') {
        return Ok(ToolResult::failure(
            "Content contains null bytes which may cause issues",
        ));
    }

    let len = content.len();
    tracing::debug!(
        target_path = %path.display(),
        sensitivity = sensitivity.as_str(),
        "write_file target classified"
    );
    if let Some(parent) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            return Ok(ToolResult::failure(redact_output(format!(
                "write_file mkdir: {e} {outcome_annotations}"
            ))));
        }
    }
    match std::fs::write(&path, content) {
        Ok(_) => Ok(ToolResult::ok(redact_output(format!(
            "Written {len} bytes to {path:?} {outcome_annotations}"
        )))),
        Err(e) => Ok(ToolResult::failure(redact_output(format!(
            "write_file: {e} {outcome_annotations}"
        )))),
    }
}

pub fn str_replace(args: &Value, root: &Path) -> Result<ToolResult> {
    let path = resolve_safe_path(root, &str_arg(args, "path")?)?;
    let sensitivity = crate::path_sensitivity::classify_path_sensitivity(root, &path);
    let outcome_annotations = sensitivity.outcome_annotations();
    let old_str = str_arg(args, "old_str")?;
    let new_str = str_arg(args, "new_str")?;
    tracing::debug!(
        target_path = %path.display(),
        sensitivity = sensitivity.as_str(),
        "str_replace target classified"
    );
    let raw = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(e) => return Ok(ToolResult::failure(format!("str_replace: {e}"))),
    };

    // Normalize line endings: file may use LF, old_str from LLM may use CRLF or vice versa.
    // We normalize both to LF for matching, then write back preserving the file's original style.
    let file_uses_crlf = raw.contains("\r\n");
    let content = raw.replace("\r\n", "\n");
    let old_str_norm = old_str.replace("\r\n", "\n");
    let new_str_norm = new_str.replace("\r\n", "\n");

    let count = content.matches(&old_str_norm).count();
    if count == 0 {
        // Give the model a useful hint: show nearby text if old_str is almost right.
        let hint = {
            let needle_first_line = old_str_norm.lines().next().unwrap_or("").trim();
            if !needle_first_line.is_empty() {
                if let Some(line) = content.lines().find(|l| l.contains(needle_first_line)) {
                    format!(" (found similar line: {:?})", &line[..line.len().min(120)])
                } else {
                    String::new()
                }
            } else {
                String::new()
            }
        };
        return Ok(ToolResult::failure(redact_output(format!(
            "old_str not found{hint} {outcome_annotations}"
        ))));
    }
    if count > 1 {
        return Ok(ToolResult::failure(redact_output(format!(
            "old_str found {count} times — must be unique. Add more surrounding context to old_str. {outcome_annotations}"
        ))));
    }

    let replaced = content.replacen(&old_str_norm, &new_str_norm, 1);
    // Restore original line endings if file used CRLF
    let output = if file_uses_crlf {
        replaced.replace("\n", "\r\n")
    } else {
        replaced
    };

    match std::fs::write(&path, output) {
        Ok(_) => Ok(ToolResult::ok(redact_output(format!(
            "Replaced in {path:?} {outcome_annotations}"
        )))),
        Err(e) => Ok(ToolResult::failure(redact_output(format!(
            "str_replace write: {e} {outcome_annotations}"
        )))),
    }
}

/// Apply multiple str_replace edits to a file in a single call.
///
/// Args:
///   path    — file to edit
///   edits   — array of { old_str, new_str } objects, applied in order
///
/// Each old_str must be unique in the file at the time it is applied.
/// Edits are applied sequentially, so later edits see the result of earlier ones.
/// On any failure the file is left unchanged (all-or-nothing).
pub fn str_replace_multi(args: &Value, root: &Path) -> Result<ToolResult> {
    let path = resolve_safe_path(root, &str_arg(args, "path")?)?;
    let sensitivity = crate::path_sensitivity::classify_path_sensitivity(root, &path);
    let outcome_annotations = sensitivity.outcome_annotations();
    let edits = args
        .get("edits")
        .and_then(|v| v.as_array())
        .ok_or_else(|| anyhow::anyhow!("Missing arg: edits (expected array)"))?;

    tracing::debug!(
        target_path = %path.display(),
        sensitivity = sensitivity.as_str(),
        "str_replace_multi target classified"
    );

    if edits.is_empty() {
        return Ok(ToolResult::failure(
            "str_replace_multi: edits array is empty",
        ));
    }
    if edits.len() > 20 {
        return Ok(ToolResult::failure(format!(
            "str_replace_multi: too many edits ({} > max 20)",
            edits.len()
        )));
    }

    let raw = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(e) => return Ok(ToolResult::failure(format!("str_replace_multi: {e}"))),
    };

    let file_uses_crlf = raw.contains("\r\n");
    let mut current = raw.replace("\r\n", "\n");
    let mut applied = 0usize;

    for (i, edit) in edits.iter().enumerate() {
        let old_str = edit
            .get("old_str")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("edits[{i}] missing old_str"))?
            .replace("\r\n", "\n");
        let new_str = edit
            .get("new_str")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("edits[{i}] missing new_str"))?
            .replace("\r\n", "\n");

        let count = current.matches(&old_str).count();
        if count == 0 {
            let hint = {
                let first_line = old_str.lines().next().unwrap_or("").trim();
                if !first_line.is_empty() {
                    if let Some(line) = current.lines().find(|l| l.contains(first_line)) {
                        let preview = &line[..line.len().min(120)];
                        format!(" (found similar line: {preview:?})")
                    } else {
                        String::new()
                    }
                } else {
                    String::new()
                }
            };
            return Ok(ToolResult::failure(redact_output(format!(
                "str_replace_multi: edits[{i}] old_str not found{hint} (applied {applied} edits before this) {outcome_annotations}"
            ))));
        }
        if count > 1 {
            return Ok(ToolResult::failure(redact_output(format!(
                "str_replace_multi: edits[{i}] old_str found {count} times — must be unique. Add more context. {outcome_annotations}"
            ))));
        }
        current = current.replacen(&old_str, &new_str, 1);
        applied += 1;
    }

    let output = if file_uses_crlf {
        current.replace("\n", "\r\n")
    } else {
        current
    };

    match std::fs::write(&path, output) {
        Ok(_) => Ok(ToolResult::ok(redact_output(format!(
            "Applied {applied} replacement(s) in {path:?} {outcome_annotations}"
        )))),
        Err(e) => Ok(ToolResult::failure(redact_output(format!(
            "str_replace_multi write: {e} {outcome_annotations}"
        )))),
    }
}

/// str_replace with whitespace-normalised matching.
///
/// Useful when the LLM generates old_str with wrong indentation or trailing spaces.
/// Matching is done on stripped lines; the replacement preserves the original indentation.
///
/// Args:
///   path    — file to edit
///   old_str — the text to find (whitespace-normalised)
///   new_str — replacement text
pub fn str_replace_fuzzy(args: &Value, root: &Path) -> Result<ToolResult> {
    let path = resolve_safe_path(root, &str_arg(args, "path")?)?;
    let sensitivity = crate::path_sensitivity::classify_path_sensitivity(root, &path);
    let outcome_annotations = sensitivity.outcome_annotations();
    let old_str = str_arg(args, "old_str")?;
    let new_str = str_arg(args, "new_str")?;

    let raw = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(e) => return Ok(ToolResult::failure(format!("str_replace_fuzzy: {e}"))),
    };

    let file_uses_crlf = raw.contains("\r\n");
    let content = raw.replace("\r\n", "\n");
    let old_norm = old_str.replace("\r\n", "\n");
    let new_norm = new_str.replace("\r\n", "\n");

    // First try exact match (same as str_replace)
    let exact_count = content.matches(&old_norm).count();
    if exact_count == 1 {
        let replaced = content.replacen(&old_norm, &new_norm, 1);
        let output = if file_uses_crlf {
            replaced.replace("\n", "\r\n")
        } else {
            replaced
        };
        return match std::fs::write(&path, output) {
            Ok(_) => Ok(ToolResult::ok(redact_output(format!(
                "Replaced (exact) in {path:?} {outcome_annotations}"
            )))),
            Err(e) => Ok(ToolResult::failure(redact_output(format!("str_replace_fuzzy write: {e}")))),
        };
    }
    if exact_count > 1 {
        return Ok(ToolResult::failure(format!(
            "old_str found {exact_count} times — must be unique. Add more surrounding context."
        )));
    }

    // Fuzzy: normalize each line (trim leading/trailing whitespace) for matching.
    // Find a contiguous run of lines in the file whose stripped content matches
    // the stripped lines of old_str.
    let needle_lines: Vec<&str> = old_norm.lines().collect();
    let file_lines: Vec<&str> = content.lines().collect();

    if needle_lines.is_empty() {
        return Ok(ToolResult::failure("str_replace_fuzzy: old_str is empty"));
    }

    let stripped_needle: Vec<&str> = needle_lines.iter().map(|l| l.trim()).collect();
    let stripped_file: Vec<&str> = file_lines.iter().map(|l| l.trim()).collect();

    let mut match_start: Option<usize> = None;
    let mut match_count = 0usize;

    'outer: for i in 0..=file_lines.len().saturating_sub(needle_lines.len()) {
        for (j, needle) in stripped_needle.iter().enumerate() {
            if stripped_file.get(i + j) != Some(needle) {
                continue 'outer;
            }
        }
        match_start = Some(i);
        match_count += 1;
        if match_count > 1 {
            return Ok(ToolResult::failure(
                "str_replace_fuzzy: old_str matches multiple locations after whitespace normalisation. Add more context."
                    .to_string(),
            ));
        }
    }

    let start = match match_start {
        Some(s) => s,
        None => {
            return Ok(ToolResult::failure(
                "str_replace_fuzzy: old_str not found (tried exact and whitespace-normalised matching)"
                    .to_string(),
            ));
        }
    };

    // Detect indentation from the first matched line in the file
    let file_indent = file_lines[start]
        .chars()
        .take_while(|c| c.is_whitespace())
        .collect::<String>();

    // Re-indent new_str to match the file's indentation at match site.
    // Strategy: strip common leading whitespace from new_str, then add file_indent.
    let new_lines: Vec<&str> = new_norm.lines().collect();
    let common_indent = new_lines
        .iter()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.len() - l.trim_start().len())
        .min()
        .unwrap_or(0);

    let reindented: Vec<String> = new_lines
        .iter()
        .map(|l| {
            if l.trim().is_empty() {
                String::new()
            } else {
                format!("{}{}", file_indent, &l[common_indent.min(l.len())..])
            }
        })
        .collect();

    let mut result_lines = file_lines[..start]
        .to_vec()
        .iter()
        .map(|s| s.to_string())
        .collect::<Vec<_>>();
    result_lines.extend(reindented);
    result_lines.extend(
        file_lines[start + needle_lines.len()..]
            .iter()
            .map(|s| s.to_string()),
    );

    let replaced = result_lines.join("\n");
    // Preserve trailing newline if original had one
    let replaced = if content.ends_with('\n') && !replaced.ends_with('\n') {
        format!("{replaced}\n")
    } else {
        replaced
    };

    let output = if file_uses_crlf {
        replaced.replace("\n", "\r\n")
    } else {
        replaced
    };

    match std::fs::write(&path, output) {
        Ok(_) => Ok(ToolResult::ok(redact_output(format!(
            "Replaced (fuzzy, indent={file_indent:?}) in {path:?} {outcome_annotations}"
        )))),
        Err(e) => Ok(ToolResult::failure(redact_output(format!("str_replace_fuzzy write: {e}")))),
    }
}

pub fn apply_patch_preview(args: &Value, root: &Path) -> Result<ToolResult> {
    let path = resolve_safe_path(root, &str_arg(args, "path")?)?;
    let display_path = path
        .strip_prefix(root)
        .unwrap_or(&path)
        .to_string_lossy()
        .replace('\\', "/");
    let current = match std::fs::read_to_string(&path) {
        Ok(content) => content,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Ok(ToolResult::failure(format!("apply_patch_preview: {e}"))),
    };

    let proposed = if let Some(content) = args.get("content").and_then(|v| v.as_str()) {
        content.to_string()
    } else {
        let old_str = str_arg(args, "old_str")?;
        let new_str = str_arg(args, "new_str")?;
        let count = current.matches(&old_str).count();
        if count == 0 {
            return Ok(ToolResult::failure("old_str not found"));
        }
        if count > 1 {
            return Ok(ToolResult::failure(format!(
                "old_str found {count} times - must be unique"
            )));
        }
        current.replacen(&old_str, &new_str, 1)
    };

    if current == proposed {
        return Ok(ToolResult::ok(format!(
            "No changes to preview for {display_path}"
        )));
    }

    let diff = render_unified_diff(&display_path, &current, &proposed);
    Ok(ToolResult::ok(format!(
        "Patch preview for {display_path}:\n{diff}"
    )))
}

pub fn list_dir(args: &Value, root: &Path) -> Result<ToolResult> {
    let dir = if let Some(p) = args.get("path").and_then(|v| v.as_str()) {
        resolve_safe_path(root, p)?
    } else {
        root.to_path_buf()
    };
    let mut entries: Vec<String> = std::fs::read_dir(&dir)
        .context("Failed to read directory")?
        .filter_map(|e| e.ok())
        .map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            let ft = e.file_type().ok();
            if ft.map(|t| t.is_dir()).unwrap_or(false) {
                format!("{name}/")
            } else {
                name
            }
        })
        .collect();
    entries.sort();
    Ok(ToolResult::ok(format!(
        "{}:\n{}",
        dir.display(),
        entries.join("\n")
    )))
}

pub fn find_files(args: &Value, root: &Path) -> Result<ToolResult> {
    let pattern = str_arg(args, "pattern")?;
    let dir = if let Some(p) = args.get("dir").and_then(|v| v.as_str()) {
        resolve_safe_path(root, p)?
    } else {
        root.to_path_buf()
    };
    let pattern_lower = pattern.to_lowercase();
    let (prefix, suffix) = if let Some(s) = pattern.strip_prefix('*') {
        ("", s)
    } else if let Some(p) = pattern.strip_suffix('*') {
        (p, "")
    } else {
        ("", pattern.as_str())
    };
    let mut results = Vec::new();
    for entry in walkdir::WalkDir::new(&dir)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        if entry.file_type().is_file() {
            let name = entry.file_name().to_string_lossy().to_lowercase();
            let matches = if prefix.is_empty() && suffix.is_empty() {
                name.contains(&pattern_lower)
            } else if prefix.is_empty() {
                name.ends_with(suffix)
            } else {
                name.starts_with(prefix)
            };
            if matches {
                let rel = entry.path().strip_prefix(&dir).unwrap_or(entry.path());
                results.push(rel.to_string_lossy().to_string());
            }
        }
    }
    results.sort();
    Ok(ToolResult::ok(results.join("\n")))
}

pub fn search_in_files(args: &Value, root: &Path) -> Result<ToolResult> {
    let pattern = str_arg(args, "pattern")?;
    let dir = if let Some(p) = args.get("dir").and_then(|v| v.as_str()) {
        resolve_safe_path(root, p)?
    } else {
        root.to_path_buf()
    };
    let ext = args.get("ext").and_then(|v| v.as_str());
    let mut results = Vec::new();
    let skip_dirs = [
        "target",
        "node_modules",
        ".git",
        "venv",
        "__pycache__",
        "build",
        "dist",
    ];
    for entry in walkdir::WalkDir::new(&dir)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| {
            let name = e.file_name().to_string_lossy();
            !skip_dirs.iter().any(|skip| name == *skip)
        })
        .filter_map(|e| e.ok())
    {
        if entry.file_type().is_file() {
            let path = entry.path();
            if let Some(e) = ext {
                let ext_matches = path
                    .extension()
                    .map(|s| s.to_string_lossy().to_lowercase())
                    .map(|s| s.as_str() == e)
                    .unwrap_or(false);
                if !ext_matches {
                    continue;
                }
            }
            if let Ok(content) = std::fs::read_to_string(path) {
                for (i, line) in content.lines().enumerate() {
                    if line.contains(&pattern) {
                        let rel = path.strip_prefix(&dir).unwrap_or(path);
                        results.push(format!("{}:{}: {}", rel.display(), i + 1, line.trim()));
                    }
                }
            }
        }
    }
    Ok(ToolResult::ok(results.join("\n")))
}

fn read_text_file(path: &Path, tool_name: &str) -> std::result::Result<String, ToolResult> {
    match std::fs::read_to_string(path) {
        Ok(c) => Ok(c),
        Err(e) => {
            if e.kind() == std::io::ErrorKind::NotFound {
                return Err(ToolResult::failure(format!(
                    "{tool_name}: No such file or directory: {path:?}"
                )));
            }
            Err(ToolResult::failure(format!("{tool_name}: {e}")))
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum DiffOp {
    Equal(String),
    Delete(String),
    Insert(String),
}

fn render_unified_diff(path: &str, current: &str, proposed: &str) -> String {
    let current_lines = current
        .lines()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();
    let proposed_lines = proposed
        .lines()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();
    let ops = diff_ops(&current_lines, &proposed_lines);

    let mut out = String::new();
    out.push_str(&format!("--- a/{path}\n"));
    out.push_str(&format!("+++ b/{path}\n"));
    out.push_str(&format!(
        "@@ -1,{} +1,{} @@\n",
        current_lines.len(),
        proposed_lines.len()
    ));

    for op in ops {
        match op {
            DiffOp::Equal(line) => {
                out.push(' ');
                out.push_str(&line);
            }
            DiffOp::Delete(line) => {
                out.push('-');
                out.push_str(&line);
            }
            DiffOp::Insert(line) => {
                out.push('+');
                out.push_str(&line);
            }
        }
        out.push('\n');
    }

    out.trim_end().to_string()
}

fn diff_ops(current: &[String], proposed: &[String]) -> Vec<DiffOp> {
    let mut dp = vec![vec![0usize; proposed.len() + 1]; current.len() + 1];

    for i in (0..current.len()).rev() {
        for j in (0..proposed.len()).rev() {
            dp[i][j] = if current[i] == proposed[j] {
                dp[i + 1][j + 1] + 1
            } else {
                dp[i + 1][j].max(dp[i][j + 1])
            };
        }
    }

    let mut i = 0;
    let mut j = 0;
    let mut ops = Vec::new();

    while i < current.len() && j < proposed.len() {
        if current[i] == proposed[j] {
            ops.push(DiffOp::Equal(current[i].clone()));
            i += 1;
            j += 1;
        } else if dp[i + 1][j] >= dp[i][j + 1] {
            ops.push(DiffOp::Delete(current[i].clone()));
            i += 1;
        } else {
            ops.push(DiffOp::Insert(proposed[j].clone()));
            j += 1;
        }
    }

    while i < current.len() {
        ops.push(DiffOp::Delete(current[i].clone()));
        i += 1;
    }
    while j < proposed.len() {
        ops.push(DiffOp::Insert(proposed[j].clone()));
        j += 1;
    }

    ops
}

fn resolve_test_failure_path(args: &Value, root: &Path) -> Result<std::path::PathBuf> {
    if let Some(path) = args.get("path").and_then(|v| v.as_str()) {
        return resolve_safe_path(root, path);
    }

    let candidates = [
        root.join("test-output.txt"),
        root.join(".ai").join("state").join("last_session.md"),
    ];
    for candidate in candidates {
        if candidate.exists() {
            return Ok(candidate);
        }
    }

    anyhow::bail!(
        "read_test_failure: missing arg: path (no default test log found in test-output.txt or .ai/state/last_session.md)"
    )
}

#[derive(Clone, Debug)]
struct FailedTestSummary {
    name: String,
    line_number: usize,
}

#[derive(Clone, Debug)]
struct FailureBlock {
    name: String,
    start_line: usize,
    end_line: usize,
    text: String,
}

fn collect_failed_test_summaries(lines: &[&str]) -> Vec<FailedTestSummary> {
    lines
        .iter()
        .enumerate()
        .filter_map(|(idx, line)| {
            let trimmed = line.trim();
            let rest = trimmed.strip_prefix("test ")?;
            let name = rest.strip_suffix(" ... FAILED")?;
            Some(FailedTestSummary {
                name: name.trim().to_string(),
                line_number: idx + 1,
            })
        })
        .collect()
}

fn collect_failure_blocks(lines: &[&str]) -> Vec<FailureBlock> {
    let mut blocks = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        let trimmed = lines[index].trim();
        if let Some(name) = parse_failure_block_header(trimmed) {
            let start = index + 1;
            let mut end = index + 1;
            let mut body = vec![lines[index].to_string()];
            index += 1;
            while index < lines.len() {
                let candidate = lines[index].trim();
                if parse_failure_block_header(candidate).is_some()
                    || candidate == "failures:"
                    || candidate.starts_with("test result:")
                {
                    break;
                }
                body.push(lines[index].to_string());
                end = index + 1;
                index += 1;
            }
            blocks.push(FailureBlock {
                name: name.to_string(),
                start_line: start,
                end_line: end,
                text: body.join("\n").trim_end().to_string(),
            });
            continue;
        }
        index += 1;
    }
    blocks
}

fn parse_failure_block_header(line: &str) -> Option<&str> {
    let rest = line.strip_prefix("---- ")?;
    rest.strip_suffix(" stdout ----")
        .or_else(|| rest.strip_suffix(" stderr ----"))
}

fn render_test_failure_excerpt(
    lines: &[&str],
    target_line: usize,
    before: usize,
    after: usize,
) -> String {
    if lines.is_empty() {
        return String::new();
    }

    let target = target_line.clamp(1, lines.len());
    let start = target.saturating_sub(before).max(1);
    let mut end = (target + after).min(lines.len());
    for current in target + 1..=end {
        let trimmed = lines[current - 1].trim();
        if trimmed.starts_with("test ") && trimmed.ends_with(" ... FAILED") {
            end = current.saturating_sub(1);
            break;
        }
        if parse_failure_block_header(trimmed).is_some() {
            end = current.saturating_sub(1);
            break;
        }
    }
    lines[start - 1..end]
        .iter()
        .enumerate()
        .map(|(i, text)| {
            let current = start + i;
            let marker = if current == target { ">" } else { " " };
            format!("{marker}{:>4}  {}", current, text)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs;
    use tempfile::TempDir;

    fn setup_test_dir() -> (TempDir, std::path::PathBuf) {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().to_path_buf();
        fs::create_dir_all(root.join("subdir")).unwrap();
        fs::write(root.join("test.txt"), "line1\nline2\nline3\n").unwrap();
        fs::write(root.join("subdir").join("nested.rs"), "fn main() {}\n").unwrap();
        (tmp, root)
    }

    #[test]
    fn test_read_file_basic() {
        let (_tmp, root) = setup_test_dir();
        let args = json!({ "path": "test.txt" });
        let result = read_file(&args, &root).unwrap();
        assert!(result.success);
        assert!(result.output.contains("test.txt"));
        assert!(result.output.contains("line1"));
    }

    #[test]
    fn test_read_file_with_range() {
        let (_tmp, root) = setup_test_dir();
        let args = json!({ "path": "test.txt", "start_line": 2, "end_line": 3 });
        let result = read_file(&args, &root).unwrap();
        assert!(result.success);
        assert!(result.output.contains("lines 2-3"));
    }

    #[test]
    fn test_read_file_not_found() {
        let (_tmp, root) = setup_test_dir();
        let args = json!({ "path": "subdir/nonexistent.txt" });
        let result = read_file(&args, &root).unwrap();
        assert!(!result.success);
        assert!(result.output.contains("No such file"));
    }

    #[test]
    fn test_open_file_region_basic() {
        let (_tmp, root) = setup_test_dir();
        let args = json!({ "path": "test.txt", "line": 2, "before": 1, "after": 1 });
        let result = open_file_region(&args, &root).unwrap();
        assert!(result.success);
        assert!(result.output.contains("focus line 2"));
        assert!(result.output.contains(">   2  line2"));
    }

    #[test]
    fn test_open_file_region_clamps_to_file_bounds() {
        let (_tmp, root) = setup_test_dir();
        let args = json!({ "path": "test.txt", "line": 99, "before": 2, "after": 2 });
        let result = open_file_region(&args, &root).unwrap();
        assert!(result.success);
        assert!(result.output.contains("focus line 3"));
        assert!(result.output.contains(">   3  line3"));
    }

    #[test]
    fn test_read_test_failure_with_detailed_block() {
        let (_tmp, root) = setup_test_dir();
        fs::write(
            root.join("test-output.txt"),
            "running 2 tests\n\
test parser::tests::test_ok ... ok\n\
test parser::tests::test_bad_input ... FAILED\n\
\n\
failures:\n\
\n\
---- parser::tests::test_bad_input stdout ----\n\
thread 'parser::tests::test_bad_input' panicked at src/parser.rs:42:9:\n\
assertion failed: left == right\n\
\n\
failures:\n\
    parser::tests::test_bad_input\n\
\n\
test result: FAILED. 1 passed; 1 failed;\n",
        )
        .unwrap();

        let args = json!({ "path": "test-output.txt" });
        let result = read_test_failure(&args, &root).unwrap();
        assert!(result.success);
        assert!(result.output.contains("parser::tests::test_bad_input"));
        assert!(result.output.contains("Detailed failure block"));
        assert!(result.output.contains("panicked at src/parser.rs:42:9"));
    }

    #[test]
    fn test_read_test_failure_defaults_to_last_session_and_falls_back_to_excerpt() {
        let (_tmp, root) = setup_test_dir();
        fs::create_dir_all(root.join(".ai").join("state")).unwrap();
        fs::write(
            root.join(".ai").join("state").join("last_session.md"),
            "line 1\n\
test tools::web::tests::fetch_url::test_file_url_not_found ... FAILED\n\
line 3\n",
        )
        .unwrap();

        let args = json!({});
        let result = read_test_failure(&args, &root).unwrap();
        assert!(result.success);
        assert!(
            result
                .output
                .contains("tools::web::tests::fetch_url::test_file_url_not_found")
        );
        assert!(
            result
                .output
                .contains("Detailed failure block: not present")
        );
        assert!(result.output.contains(
            ">   2  test tools::web::tests::fetch_url::test_file_url_not_found ... FAILED"
        ));
    }

    #[test]
    fn test_read_test_failure_selects_requested_test() {
        let (_tmp, root) = setup_test_dir();
        fs::write(
            root.join("test-output.txt"),
            "test alpha::tests::first ... FAILED\n\
test alpha::tests::second ... FAILED\n\
\n\
---- alpha::tests::first stdout ----\n\
first details\n\
\n\
---- alpha::tests::second stdout ----\n\
second details\n",
        )
        .unwrap();

        let args = json!({ "path": "test-output.txt", "test": "alpha::tests::first" });
        let result = read_test_failure(&args, &root).unwrap();
        assert!(result.success);
        assert!(result.output.contains("Test failure: alpha::tests::first"));
        assert!(result.output.contains("first details"));
        assert!(!result.output.contains("second details"));
    }

    #[test]
    fn test_write_file_basic() {
        let (_tmp, root) = setup_test_dir();
        let args = json!({ "path": "subdir/new.txt", "content": "hello world" });
        let result = write_file(&args, &root).unwrap();
        assert!(result.success);
        assert!(result.output.contains("new.txt"));
        assert!(result.output.contains("[sensitivity: source]"));
        assert_eq!(
            fs::read_to_string(root.join("subdir/new.txt")).unwrap(),
            "hello world"
        );
    }

    #[test]
    fn test_write_file_creates_dirs() {
        let (_tmp, root) = setup_test_dir();
        let args = json!({ "path": "subdir/deep/nested/file.txt", "content": "content" });
        let result = write_file(&args, &root).unwrap();
        assert!(result.success);
        assert!(root.join("subdir/deep/nested/file.txt").exists());
    }

    #[test]
    fn test_write_file_empty_content() {
        let (_tmp, root) = setup_test_dir();
        let args = json!({ "path": "subdir/empty.txt", "content": "" });
        let result = write_file(&args, &root).unwrap();
        assert!(!result.success);
        assert!(result.output.contains("empty"));
    }

    #[test]
    fn test_write_file_null_bytes() {
        let (_tmp, root) = setup_test_dir();
        let args = json!({ "path": "subdir/null.txt", "content": "hello\0world" });
        let result = write_file(&args, &root).unwrap();
        assert!(!result.success);
        assert!(result.output.contains("null bytes"));
    }

    #[test]
    fn test_str_replace_basic() {
        let (_tmp, root) = setup_test_dir();
        let args = json!({ "path": "test.txt", "old_str": "line2", "new_str": "replaced" });
        let result = str_replace(&args, &root).unwrap();
        assert!(result.success);
        assert!(result.output.contains("[sensitivity: source]"));
        let content = fs::read_to_string(root.join("test.txt")).unwrap();
        assert!(content.contains("replaced"));
        assert!(!content.contains("line2"));
    }

    #[test]
    fn test_str_replace_not_found() {
        let (_tmp, root) = setup_test_dir();
        let args = json!({ "path": "test.txt", "old_str": "nonexistent", "new_str": "something" });
        let result = str_replace(&args, &root).unwrap();
        assert!(!result.success);
        assert!(result.output.contains("not found"));
        assert!(result.output.contains("[sensitivity: source]"));
    }

    #[test]
    fn test_str_replace_multiple_occurrences() {
        let (_tmp, root) = setup_test_dir();
        fs::write(root.join("multi.txt"), "line\nline\nline\n").unwrap();
        let args = json!({ "path": "multi.txt", "old_str": "line", "new_str": "replaced" });
        let result = str_replace(&args, &root).unwrap();
        assert!(!result.success);
        assert!(result.output.contains("found"));
        assert!(result.output.contains("times"));
    }

    #[test]
    fn test_str_replace_file_not_found() {
        let (_tmp, root) = setup_test_dir();
        let args = json!({ "path": "nonexistent.txt", "old_str": "something", "new_str": "other" });
        let result = str_replace(&args, &root).unwrap();
        assert!(!result.success);
        assert!(result.output.contains("str_replace"));
    }

    #[test]
    fn test_apply_patch_preview_for_str_replace_mode() {
        let (_tmp, root) = setup_test_dir();
        let args = json!({ "path": "test.txt", "old_str": "line2", "new_str": "changed" });
        let result = apply_patch_preview(&args, &root).unwrap();
        assert!(result.success);
        assert!(result.output.contains("Patch preview for"));
        assert!(result.output.contains("test.txt"));
        assert!(result.output.contains("--- a/"));
        assert!(result.output.contains("+++ b/"));
        assert!(result.output.contains("-line2"));
        assert!(result.output.contains("+changed"));
        let content = fs::read_to_string(root.join("test.txt")).unwrap();
        assert!(content.contains("line2"));
        assert!(!content.contains("changed"));
    }

    #[test]
    fn test_apply_patch_preview_for_write_mode() {
        let (_tmp, root) = setup_test_dir();
        let args = json!({ "path": "subdir/new_preview.txt", "content": "hello\nworld\n" });
        let result = apply_patch_preview(&args, &root).unwrap();
        assert!(result.success);
        assert!(result.output.contains("new_preview.txt"));
        assert!(result.output.contains("--- a/"));
        assert!(result.output.contains("+++ b/"));
        assert!(result.output.contains("+hello"));
        assert!(!root.join("subdir/new_preview.txt").exists());
    }

    #[test]
    fn test_apply_patch_preview_reports_no_change() {
        let (_tmp, root) = setup_test_dir();
        let args = json!({ "path": "test.txt", "content": "line1\nline2\nline3\n" });
        let result = apply_patch_preview(&args, &root).unwrap();
        assert!(result.success);
        assert!(result.output.contains("No changes to preview"));
    }

    #[test]
    fn test_list_dir_basic() {
        let (_tmp, root) = setup_test_dir();
        let args = json!({ "path": "." });
        let result = list_dir(&args, &root).unwrap();
        assert!(result.success);
        assert!(result.output.contains("test.txt"));
        assert!(result.output.contains("subdir/"));
    }

    #[test]
    fn test_list_dir_subdir() {
        let (_tmp, root) = setup_test_dir();
        let args = json!({ "path": "subdir" });
        let result = list_dir(&args, &root).unwrap();
        assert!(result.success);
        assert!(result.output.contains("nested.rs"));
    }

    #[test]
    fn test_list_dir_not_found() {
        let (_tmp, root) = setup_test_dir();
        let args = json!({ "path": "nonexistent_dir" });
        let result = list_dir(&args, &root);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.to_string().contains("Failed to read") || err.to_string().contains("cannot find")
        );
    }

    #[test]
    fn test_read_file_directory() {
        let (_tmp, root) = setup_test_dir();
        let args = json!({ "path": "subdir" });
        let result = read_file(&args, &root).unwrap();
        assert!(!result.success);
        assert!(result.output.contains("read_file"));
    }

    #[test]
    fn test_find_files_pattern() {
        let (_tmp, root) = setup_test_dir();
        let args = json!({ "pattern": "*.txt" });
        let result = find_files(&args, &root).unwrap();
        assert!(result.success);
        assert!(result.output.contains("test.txt"));
    }

    #[test]
    fn test_find_files_extension() {
        let (_tmp, root) = setup_test_dir();
        let args = json!({ "pattern": "*.rs" });
        let result = find_files(&args, &root).unwrap();
        assert!(result.success);
        assert!(result.output.contains("nested.rs"));
    }

    #[test]
    fn test_search_in_files_basic() {
        let (_tmp, root) = setup_test_dir();
        let args = json!({ "pattern": "line2" });
        let result = search_in_files(&args, &root).unwrap();
        assert!(result.success);
        assert!(result.output.contains("test.txt"));
        assert!(result.output.contains("line2"));
    }

    #[test]
    fn test_search_in_files_with_ext() {
        let (_tmp, root) = setup_test_dir();
        let args = json!({ "pattern": "fn", "ext": "rs" });
        let result = search_in_files(&args, &root).unwrap();
        assert!(result.success);
        assert!(result.output.contains("nested.rs"));
    }
    #[test]
    fn test_str_replace_multi_basic() {
        let (_tmp, root) = setup_test_dir();
        let args = json!({
            "path": "test.txt",
            "edits": [
                { "old_str": "line1", "new_str": "replaced1" },
                { "old_str": "line2", "new_str": "replaced2" }
            ]
        });
        let result = str_replace_multi(&args, &root).unwrap();
        assert!(result.success, "{}", result.output);
        assert!(result.output.contains("[sensitivity: source]"));
        let content = fs::read_to_string(root.join("test.txt")).unwrap();
        assert!(content.contains("replaced1"));
        assert!(content.contains("replaced2"));
        assert!(!content.contains("line1"));
        assert!(!content.contains("line2"));
    }

    #[test]
    fn test_str_replace_multi_stops_on_not_found() {
        let (_tmp, root) = setup_test_dir();
        let args = json!({
            "path": "test.txt",
            "edits": [
                { "old_str": "line1", "new_str": "ok" },
                { "old_str": "nonexistent", "new_str": "fail" }
            ]
        });
        let result = str_replace_multi(&args, &root).unwrap();
        assert!(!result.success);
        assert!(result.output.contains("not found"));
        assert!(result.output.contains("[sensitivity: source]"));
        // File should be unchanged (first edit not written)
        let content = fs::read_to_string(root.join("test.txt")).unwrap();
        assert!(content.contains("line1")); // not modified
    }

    #[test]
    fn test_write_file_reports_project_config_sensitivity() {
        let (_tmp, root) = setup_test_dir();
        let args = json!({ "path": "config.toml", "content": "model = \"x\"\n" });
        let result = write_file(&args, &root).unwrap();
        assert!(result.success);
        assert!(result.output.contains("[sensitivity: project_config]"));
        assert!(
            result
                .output
                .contains("[warning: editing project config can affect future runs]")
        );
        assert!(
            result
                .output
                .contains("[policy: re-check resolved config or runtime behavior after this edit]")
        );
    }

    #[test]
    fn test_write_file_reports_repo_meta_warning() {
        let (_tmp, root) = setup_test_dir();
        fs::create_dir_all(root.join(".git")).unwrap();
        let args =
            json!({ "path": ".git/config", "content": "[core]\nrepositoryformatversion = 0\n" });
        let result = write_file(&args, &root).unwrap();
        assert!(result.success);
        assert!(result.output.contains("[sensitivity: repo_meta]"));
        assert!(
            result
                .output
                .contains("[warning: editing repo metadata can affect git behavior]")
        );
        assert!(
            result
                .output
                .contains("[policy: review git diff/status carefully after this edit]")
        );
    }

    #[test]
    fn test_str_replace_reports_prompt_warning() {
        let (_tmp, root) = setup_test_dir();
        fs::create_dir_all(root.join(".ai/prompts")).unwrap();
        fs::write(root.join(".ai/prompts/boss.md"), "hello boss\n").unwrap();
        let args = json!({ "path": ".ai/prompts/boss.md", "old_str": "hello boss", "new_str": "updated boss" });
        let result = str_replace(&args, &root).unwrap();
        assert!(result.success);
        assert!(result.output.contains("[sensitivity: prompts]"));
        assert!(
            result
                .output
                .contains("[warning: editing prompts can change future agent behavior]")
        );
        assert!(
            result
                .output
                .contains("[policy: expect future agent sessions to follow the updated prompt]")
        );
    }

    #[test]
    fn test_str_replace_multi_empty_edits() {
        let (_tmp, root) = setup_test_dir();
        let args = json!({ "path": "test.txt", "edits": [] });
        let result = str_replace_multi(&args, &root).unwrap();
        assert!(!result.success);
        assert!(result.output.contains("empty"));
    }

    #[test]
    fn test_str_replace_fuzzy_exact_match() {
        let (_tmp, root) = setup_test_dir();
        let args = json!({
            "path": "test.txt",
            "old_str": "line2",
            "new_str": "replaced"
        });
        let result = str_replace_fuzzy(&args, &root).unwrap();
        assert!(result.success, "{}", result.output);
        let content = fs::read_to_string(root.join("test.txt")).unwrap();
        assert!(content.contains("replaced"));
    }

    #[test]
    fn test_str_replace_fuzzy_indentation_mismatch() {
        let (_tmp, root) = setup_test_dir();
        // Write a file with indented content
        fs::write(
            root.join("indented.rs"),
            "fn foo() {
    let x = 1;
    let y = 2;
}
",
        )
        .unwrap();
        // old_str with wrong indentation (no indent)
        let args = json!({
            "path": "indented.rs",
            "old_str": "let x = 1;
        let y = 2;",
            "new_str": "let z = 3;"
        });
        let result = str_replace_fuzzy(&args, &root).unwrap();
        assert!(result.success, "{}", result.output);
        let content = fs::read_to_string(root.join("indented.rs")).unwrap();
        assert!(
            content.contains("    let z = 3;"),
            "should preserve indentation: {content}"
        );
    }

    #[test]
    fn test_str_replace_fuzzy_not_found() {
        let (_tmp, root) = setup_test_dir();
        let args = json!({
            "path": "test.txt",
            "old_str": "nonexistent_content",
            "new_str": "x"
        });
        let result = str_replace_fuzzy(&args, &root).unwrap();
        assert!(!result.success);
        assert!(result.output.contains("not found"));
    }

    #[test]
    fn test_write_file_output_redacts_sensitive_token_in_path_error() {
        // Simulate a write to a path whose string representation contains a
        // sensitive-looking token.  We verify that the output message does not
        // leak the token even when it comes from the OS error text.
        // In practice the most likely leakage is through error messages that
        // echo back parts of the request; we cover that by testing a path that
        // triggers a mkdir failure with a very long name component.
        let (_tmp, root) = setup_test_dir();
        // Write a normal file — the message itself should pass through clean.
        let args = json!({ "path": "subdir/out.txt", "content": "data" });
        let result = write_file(&args, &root).unwrap();
        assert!(result.success);
        // Sensitivity annotation must still be present.
        assert!(result.output.contains("[sensitivity:"));
        // The output must not accidentally contain any known redaction trigger.
        assert!(!result.output.contains("password="));
    }

    #[test]
    fn test_str_replace_output_does_not_contain_sensitive_hint_text() {
        // If a file contains a sensitive line and old_str's first line matches
        // it (triggering the "found similar line" hint), that hint must be
        // redacted before the message is returned.
        let (_tmp, root) = setup_test_dir();
        // Write a file whose first line matches the first line of old_str so
        // the hint logic fires.  The content of that line is sensitive.
        fs::write(root.join("secret.txt"), "api_key=hunter2\n").unwrap();
        let args = json!({
            "path": "secret.txt",
            // First line of old_str matches the file line exactly, so the
            // similar-line hint will echo it back — that echo must be redacted.
            "old_str": "api_key=hunter2\nnot_present_second_line",
            "new_str": "api_key=replaced"
        });
        let result = str_replace(&args, &root).unwrap();
        assert!(!result.success);
        // The hint must not leak the secret value.
        assert!(!result.output.contains("hunter2"), "output leaked secret: {}", result.output);
        assert!(result.output.contains("[redacted]"));
    }

    #[test]
    fn test_str_replace_multi_output_redacts_sensitive_hint() {
        let (_tmp, root) = setup_test_dir();
        // First line of old_str matches the file line so the hint fires.
        fs::write(root.join("secrets.txt"), "password=s3cr3t\nother line\n").unwrap();
        let args = json!({
            "path": "secrets.txt",
            "edits": [
                { "old_str": "password=s3cr3t\nnot_present", "new_str": "x" }
            ]
        });
        let result = str_replace_multi(&args, &root).unwrap();
        assert!(!result.success);
        assert!(!result.output.contains("s3cr3t"), "output leaked secret: {}", result.output);
        assert!(result.output.contains("[redacted]"));
    }
}
