use super::core::ToolResult;
use crate::validation::resolve_safe_path;
use anyhow::Result;
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use tokio::process::Command;

pub async fn diff_repo(args: &Value, root: &Path) -> Result<ToolResult> {
    let cwd = if let Some(p) = args.get("cwd").and_then(|v| v.as_str()) {
        resolve_safe_path(root, p)?
    } else {
        root.to_path_buf()
    };
    let timeout_secs = args
        .get("timeout_secs")
        .and_then(|v| v.as_u64())
        .unwrap_or(60);
    let staged = args
        .get("staged")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let stat = args.get("stat").and_then(|v| v.as_bool()).unwrap_or(false);
    let base = args
        .get("base")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let pathspec = args
        .get("path")
        .or_else(|| args.get("dir"))
        .and_then(|v| v.as_str())
        .map(|p| resolve_safe_path(&cwd, p))
        .transpose()?;

    let mut cmd = Command::new("git");
    cmd.current_dir(&cwd).arg("diff");
    if staged {
        cmd.arg("--cached");
    }
    if stat {
        cmd.arg("--stat");
    }
    if let Some(base) = &base {
        cmd.arg(base);
    }
    if let Some(pathspec) = &pathspec {
        let rel = pathspec.strip_prefix(&cwd).unwrap_or(pathspec);
        cmd.arg("--").arg(rel);
    }

    let output =
        tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), cmd.output()).await;

    match output {
        Ok(Ok(out)) => {
            let stdout = String::from_utf8_lossy(&out.stdout).to_string();
            let stderr = String::from_utf8_lossy(&out.stderr).to_string();
            if out.status.success() {
                if stdout.trim().is_empty() {
                    Ok(ToolResult::ok("No diff".to_string()))
                } else {
                    Ok(ToolResult::ok(stdout))
                }
            } else {
                Ok(ToolResult {
                    output: format!("git diff failed: {}", stderr.trim()),
                    success: false,
                })
            }
        }
        Ok(Err(e)) => Ok(ToolResult {
            output: format!("git diff: {}", e),
            success: false,
        }),
        Err(_) => Ok(ToolResult {
            output: format!("git diff: timeout after {timeout_secs}s"),
            success: false,
        }),
    }
}

pub async fn tree(_args: &Value, root: &Path) -> Result<ToolResult> {
    let mut output = String::from("Tree:\n");
    if let Ok(entries) = fs::read_dir(root) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if !name.starts_with('.') {
                output.push_str(&format!("  {}\n", name));
            }
        }
    }
    Ok(ToolResult::ok(output))
}

pub async fn project_map(args: &Value, root: &Path) -> Result<ToolResult> {
    let dir = if let Some(path) = args
        .get("dir")
        .or_else(|| args.get("path"))
        .and_then(|v| v.as_str())
    {
        resolve_safe_path(root, path)?
    } else {
        root.to_path_buf()
    };
    let max_depth = args
        .get("depth")
        .and_then(|v| v.as_u64())
        .map(|n| n as usize)
        .unwrap_or(3);

    let mut top_level_dirs = Vec::new();
    let mut top_level_files = Vec::new();
    if let Ok(entries) = fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with('.') {
                continue;
            }
            let path = entry.path();
            if path.is_dir() {
                top_level_dirs.push(name);
            } else {
                top_level_files.push(name);
            }
        }
    }
    top_level_dirs.sort();
    top_level_files.sort();

    let mut extension_counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut source_roots: BTreeMap<String, usize> = BTreeMap::new();
    let mut total_files = 0usize;
    let mut total_dirs = 0usize;
    let skip_dirs = [
        "target",
        "node_modules",
        ".git",
        "dist",
        "build",
        "__pycache__",
        "venv",
    ];

    for entry in walkdir::WalkDir::new(&dir)
        .follow_links(false)
        .max_depth(max_depth)
        .into_iter()
        .filter_entry(|entry| {
            let name = entry.file_name().to_string_lossy();
            !skip_dirs.iter().any(|skip| name == *skip)
        })
        .filter_map(|entry| entry.ok())
    {
        if entry.depth() == 0 {
            continue;
        }
        if entry.file_type().is_dir() {
            total_dirs += 1;
            continue;
        }
        if entry.file_type().is_file() {
            total_files += 1;
            let ext = entry
                .path()
                .extension()
                .and_then(|ext| ext.to_str())
                .map(|ext| format!(".{ext}"))
                .unwrap_or_else(|| "[no extension]".to_string());
            *extension_counts.entry(ext).or_insert(0) += 1;

            let rel = entry.path().strip_prefix(&dir).unwrap_or(entry.path());
            let mut components = rel.components();
            if let (Some(first), Some(_second)) = (components.next(), components.next()) {
                let root_name = first.as_os_str().to_string_lossy().to_string();
                *source_roots.entry(root_name).or_insert(0) += 1;
            }
        }
    }

    let key_manifests = [
        "Cargo.toml",
        "Cargo.lock",
        "package.json",
        "tsconfig.json",
        "pyproject.toml",
        "go.mod",
        "README.md",
        "DOCS.md",
        "TODO.md",
    ]
    .into_iter()
    .filter(|name| dir.join(name).exists())
    .collect::<Vec<_>>();

    let extension_summary = summarize_map(&extension_counts, 6);
    let source_root_summary = summarize_map(&source_roots, 6);

    let mut output = format!(
        "Project map: {}\nScanned depth: {}\nTop-level directories: {}\nTop-level files: {}\nKey manifests: {}\nScanned totals: {} dirs, {} files\nLikely source roots: {}\nFile types: {}",
        dir.display(),
        max_depth,
        format_list(&top_level_dirs, 8),
        format_list(&top_level_files, 8),
        if key_manifests.is_empty() { "(none)".to_string() } else { key_manifests.join(", ") },
        total_dirs,
        total_files,
        if source_root_summary.is_empty() { "(none)".to_string() } else { source_root_summary },
        if extension_summary.is_empty() { "(none)".to_string() } else { extension_summary },
    );

    if let Some(src_files) = source_roots.get("src") {
        output.push_str(&format!("\nProject hint: `src/` appears to be the main source root ({} files within scanned depth).", src_files));
    } else if let Some(lib_files) = source_roots.get("lib") {
        output.push_str(&format!("\nProject hint: `lib/` appears to be a primary code root ({} files within scanned depth).", lib_files));
    }

    Ok(ToolResult::ok(output))
}

pub async fn find_entrypoints(args: &Value, root: &Path) -> Result<ToolResult> {
    let dir = if let Some(path) = args
        .get("dir")
        .or_else(|| args.get("path"))
        .and_then(|v| v.as_str())
    {
        resolve_safe_path(root, path)?
    } else {
        root.to_path_buf()
    };
    let max_depth = args
        .get("depth")
        .and_then(|v| v.as_u64())
        .map(|n| n as usize)
        .unwrap_or(4);
    let limit = args
        .get("limit")
        .and_then(|v| v.as_u64())
        .map(|n| n as usize)
        .unwrap_or(12);
    let skip_dirs = [
        "target",
        "node_modules",
        ".git",
        "dist",
        "build",
        "__pycache__",
        "venv",
    ];
    let mut hits = Vec::new();

    for entry in walkdir::WalkDir::new(&dir)
        .follow_links(false)
        .max_depth(max_depth)
        .into_iter()
        .filter_entry(|entry| {
            let name = entry.file_name().to_string_lossy();
            !skip_dirs.iter().any(|skip| name == *skip)
        })
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_type().is_file())
    {
        let path = entry.path();
        let rel = normalize_display_path(path.strip_prefix(&dir).unwrap_or(path));
        let content = match fs::read_to_string(path) {
            Ok(content) => content,
            Err(_) => continue,
        };

        for candidate in detect_entrypoints_in_file(&rel, &content) {
            hits.push(candidate);
        }
    }

    if hits.is_empty() {
        return Ok(ToolResult {
            output: format!(
                "No entrypoints found under {} (depth {})",
                dir.display(),
                max_depth
            ),
            success: false,
        });
    }

    hits.sort_by(|left, right| {
        right
            .score
            .cmp(&left.score)
            .then_with(|| left.file.cmp(&right.file))
            .then_with(|| left.line.cmp(&right.line))
    });

    let total = hits.len();
    let mut out = format!(
        "Entrypoints in {} (showing {} of {}, depth {}):\n",
        dir.display(),
        total.min(limit),
        total,
        max_depth
    );

    for hit in hits.iter().take(limit) {
        out.push_str(&format!(
            "- [{}] {}:{}  {}\n  {}\n",
            hit.kind, hit.file, hit.line, hit.label, hit.snippet
        ));
    }

    if let Some(primary) = hits.first() {
        out.push_str(&format!(
            "\nPrimary hint: start with {}:{} ({})",
            primary.file, primary.line, primary.label
        ));
    }

    Ok(ToolResult::ok(out.trim_end().to_string()))
}

pub async fn trace_call_path(args: &Value, root: &Path) -> Result<ToolResult> {
    let symbol = args
        .get("symbol")
        .or_else(|| args.get("name"))
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow::anyhow!("Missing arg: symbol"))?;
    let dir = if let Some(path) = args
        .get("dir")
        .or_else(|| args.get("path"))
        .and_then(|v| v.as_str())
    {
        resolve_safe_path(root, path)?
    } else {
        root.to_path_buf()
    };
    let max_depth = args
        .get("depth")
        .and_then(|v| v.as_u64())
        .map(|n| n as usize)
        .unwrap_or(3);
    let search_depth = args
        .get("search_depth")
        .and_then(|v| v.as_u64())
        .map(|n| n as usize)
        .unwrap_or(5);

    let files = collect_source_files(&dir, search_depth);
    let definitions = find_symbol_definitions(&files, symbol);
    if definitions.is_empty() {
        return Ok(ToolResult {
            output: format!(
                "trace_call_path: no definition found for '{symbol}' under {}",
                dir.display()
            ),
            success: false,
        });
    }

    let mut out = format!(
        "Call path trace for '{}' in {} (depth {}):\n",
        symbol,
        dir.display(),
        max_depth
    );

    for definition in definitions.iter().take(3) {
        out.push_str(&format!(
            "- target: {}:{}  {}\n",
            definition.file, definition.line, definition.signature
        ));
        let tree = build_call_tree(&files, symbol, max_depth);
        if tree.is_empty() {
            out.push_str("  callers: none found\n");
        } else {
            for line in tree {
                out.push_str(&format!("  {line}\n"));
            }
        }
    }

    Ok(ToolResult::ok(out.trim_end().to_string()))
}

fn format_list(items: &[String], limit: usize) -> String {
    if items.is_empty() {
        return "(none)".to_string();
    }
    if items.len() <= limit {
        return items.join(", ");
    }
    let remaining = items.len() - limit;
    format!("{}, +{} more", items[..limit].join(", "), remaining)
}

fn summarize_map(map: &BTreeMap<String, usize>, limit: usize) -> String {
    let mut items = map
        .iter()
        .map(|(name, count)| (name.clone(), *count))
        .collect::<Vec<_>>();
    items.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
    items
        .into_iter()
        .take(limit)
        .map(|(name, count)| format!("{name} ({count})"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn normalize_display_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

#[derive(Clone, Debug)]
struct EntrypointHit {
    file: String,
    line: usize,
    kind: &'static str,
    label: &'static str,
    snippet: String,
    score: u8,
}

fn detect_entrypoints_in_file(rel: &str, content: &str) -> Vec<EntrypointHit> {
    let mut hits = Vec::new();
    for (index, line) in content.lines().enumerate() {
        let trimmed = line.trim();
        let line_nr = index + 1;

        if trimmed.contains("#[tokio::main]") {
            hits.push(entrypoint_hit(
                rel,
                line_nr,
                "runtime",
                "tokio main attribute",
                trimmed,
                100,
            ));
        }
        if trimmed.starts_with("fn main(")
            || trimmed.contains(" fn main(")
            || trimmed.starts_with("async fn main(")
        {
            hits.push(entrypoint_hit(
                rel,
                line_nr,
                "main",
                "main function",
                trimmed,
                95,
            ));
        }
        if rel.ends_with("src/main.rs")
            && (trimmed.contains("clap::Parser")
                || trimmed.contains("Cli::parse()")
                || trimmed.contains("Args::parse()"))
        {
            hits.push(entrypoint_hit(
                rel,
                line_nr,
                "cli",
                "CLI bootstrap",
                trimmed,
                90,
            ));
        }
        if trimmed.contains("axum::Router") || trimmed.contains("Router::new()") {
            hits.push(entrypoint_hit(
                rel,
                line_nr,
                "web",
                "HTTP router setup",
                trimmed,
                80,
            ));
        }
        if trimmed.contains("actix_web::HttpServer") || trimmed.contains("HttpServer::new") {
            hits.push(entrypoint_hit(
                rel,
                line_nr,
                "web",
                "Actix server startup",
                trimmed,
                80,
            ));
        }
        if trimmed.contains("warp::serve(")
            || trimmed.contains("rocket::build(")
            || trimmed.contains("#[launch]")
        {
            hits.push(entrypoint_hit(
                rel,
                line_nr,
                "web",
                "web server startup",
                trimmed,
                80,
            ));
        }
        if (rel.contains("/bin/") || rel.contains("\\bin\\"))
            && (trimmed.starts_with("fn main(") || trimmed.starts_with("async fn main("))
        {
            hits.push(entrypoint_hit(
                rel,
                line_nr,
                "bin",
                "binary entrypoint",
                trimmed,
                92,
            ));
        }
        if (rel.starts_with("tests/") || rel.contains("/tests/") || rel.contains("\\tests\\"))
            && (trimmed.starts_with("#[test]") || trimmed.starts_with("#[tokio::test]"))
        {
            hits.push(entrypoint_hit(
                rel,
                line_nr,
                "test",
                "test entrypoint",
                trimmed,
                60,
            ));
        }
    }

    dedupe_entrypoints(hits)
}

fn entrypoint_hit(
    file: &str,
    line: usize,
    kind: &'static str,
    label: &'static str,
    snippet: &str,
    score: u8,
) -> EntrypointHit {
    EntrypointHit {
        file: file.to_string(),
        line,
        kind,
        label,
        snippet: snippet.to_string(),
        score,
    }
}

fn dedupe_entrypoints(hits: Vec<EntrypointHit>) -> Vec<EntrypointHit> {
    let mut deduped = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for hit in hits {
        let key = (hit.file.clone(), hit.line, hit.kind, hit.label);
        if seen.insert(key) {
            deduped.push(hit);
        }
    }
    deduped
}

#[derive(Clone, Debug)]
struct SymbolDefinition {
    file: String,
    line: usize,
    signature: String,
}

#[derive(Clone, Debug)]
struct SourceFile {
    display_path: String,
    lines: Vec<String>,
    functions: Vec<FunctionSpan>,
}

#[derive(Clone, Debug)]
struct FunctionSpan {
    name: String,
    start_line: usize,
    end_line: usize,
    signature: String,
}

fn collect_source_files(root: &Path, max_depth: usize) -> Vec<SourceFile> {
    let skip_dirs = [
        "target",
        "node_modules",
        ".git",
        "dist",
        "build",
        "__pycache__",
        "venv",
    ];
    walkdir::WalkDir::new(root)
        .follow_links(false)
        .max_depth(max_depth)
        .into_iter()
        .filter_entry(|entry| {
            let name = entry.file_name().to_string_lossy();
            !skip_dirs.iter().any(|skip| name == *skip)
        })
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_type().is_file())
        .filter_map(|entry| {
            let path = entry.path();
            let ext = path.extension().and_then(|ext| ext.to_str())?;
            if !matches!(ext, "rs" | "py" | "ts" | "tsx" | "js" | "jsx") {
                return None;
            }
            let content = fs::read_to_string(path).ok()?;
            let lines = content
                .lines()
                .map(|line| line.to_string())
                .collect::<Vec<_>>();
            let display_path = normalize_display_path(path.strip_prefix(root).unwrap_or(path));
            let functions = extract_function_spans(&lines);
            Some(SourceFile {
                display_path,
                lines,
                functions,
            })
        })
        .collect()
}

fn extract_function_spans(lines: &[String]) -> Vec<FunctionSpan> {
    let mut functions = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        if let Some(name) = detect_function_name(line) {
            let start_line = index + 1;
            let mut end_line = start_line;
            let mut brace_balance =
                line.matches('{').count() as isize - line.matches('}').count() as isize;
            if brace_balance > 0 {
                for (offset, next_line) in lines.iter().enumerate().skip(index + 1) {
                    brace_balance += next_line.matches('{').count() as isize
                        - next_line.matches('}').count() as isize;
                    end_line = offset + 1;
                    if brace_balance <= 0 {
                        break;
                    }
                }
            }
            functions.push(FunctionSpan {
                name,
                start_line,
                end_line,
                signature: line.trim().to_string(),
            });
        }
    }
    functions
}

fn detect_function_name(line: &str) -> Option<String> {
    let trimmed = line.trim();
    let prefixes = [
        "fn ",
        "async fn ",
        "pub fn ",
        "pub async fn ",
        "def ",
        "function ",
    ];
    for prefix in prefixes {
        if let Some(rest) = trimmed.strip_prefix(prefix) {
            return rest
                .split(|ch: char| ch == '(' || ch.is_whitespace() || ch == '<')
                .next()
                .filter(|name| !name.is_empty())
                .map(|name| name.to_string());
        }
    }
    None
}

fn find_symbol_definitions(files: &[SourceFile], symbol: &str) -> Vec<SymbolDefinition> {
    let mut defs = Vec::new();
    for file in files {
        for function in &file.functions {
            if function.name == symbol {
                defs.push(SymbolDefinition {
                    file: file.display_path.clone(),
                    line: function.start_line,
                    signature: function.signature.clone(),
                });
            }
        }
    }
    defs
}

fn build_call_tree(files: &[SourceFile], target_symbol: &str, max_depth: usize) -> Vec<String> {
    let mut visited = std::collections::BTreeSet::new();
    let mut out = Vec::new();
    append_callers(files, target_symbol, max_depth, 0, &mut visited, &mut out);
    out
}

fn append_callers(
    files: &[SourceFile],
    target_symbol: &str,
    max_depth: usize,
    level: usize,
    visited: &mut std::collections::BTreeSet<String>,
    out: &mut Vec<String>,
) {
    if level >= max_depth {
        return;
    }
    let callers = find_callers(files, target_symbol);
    for caller in callers {
        let caller_key = format!("{}:{}:{}", caller.file, caller.line, caller.symbol);
        if !visited.insert(caller_key) {
            continue;
        }
        let indent = "  ".repeat(level);
        out.push(format!(
            "{}<- {}:{}  {}",
            indent, caller.file, caller.line, caller.symbol
        ));
        append_callers(files, &caller.symbol, max_depth, level + 1, visited, out);
    }
}

#[derive(Clone, Debug)]
struct CallerHit {
    file: String,
    line: usize,
    symbol: String,
}

fn find_callers(files: &[SourceFile], target_symbol: &str) -> Vec<CallerHit> {
    let mut callers = Vec::new();
    for file in files {
        for function in &file.functions {
            let start = function.start_line.saturating_sub(1);
            let end = function.end_line.min(file.lines.len());
            let body = file.lines[start..end].join("\n");
            if (body.contains(&format!("{target_symbol}("))
                || body.contains(&format!(".{target_symbol}(")))
                && function.name != target_symbol
            {
                callers.push(CallerHit {
                    file: file.display_path.clone(),
                    line: function.start_line,
                    symbol: function.name.clone(),
                });
            }
        }
    }
    callers.sort_by(|left, right| {
        left.file
            .cmp(&right.file)
            .then_with(|| left.line.cmp(&right.line))
    });
    callers.dedup_by(|left, right| {
        left.file == right.file && left.line == right.line && left.symbol == right.symbol
    });
    callers
}
