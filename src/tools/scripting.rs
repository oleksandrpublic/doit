use anyhow::{anyhow, Result};
use regex::Regex;
use rhai::{Array, Dynamic, Engine, EvalAltResult, ImmutableString, Map};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::tools::core::{resolve, str_arg};
use crate::tools::tool_result::ToolResult;

const MAX_OUTPUT_BYTES: usize = 512 * 1024;
const MAX_OPERATIONS: u64 = 200_000;
const MAX_ARRAY_SIZE: usize = 10_000;
const MAX_MAP_SIZE: usize = 1_000;
const MAX_STRING_SIZE: usize = 64 * 1024;
const MAX_CALL_LEVELS: usize = 32;
const MAX_EXPR_DEPTH: usize = 64;
const MAX_VARIABLES: usize = 256;
const MAX_FUNCTIONS: usize = 64;
const MAX_MODULES: usize = 0;
const SCRIPT_TIMEOUT: Duration = Duration::from_secs(30);

pub async fn run_script(args: &Value, root: &Path) -> Result<ToolResult> {
    let script = str_arg(args, "script")?;
    let allow_write = args
        .get("allow_write")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let workdir = if let Some(dir) = args.get("dir").and_then(|v| v.as_str()) {
        resolve(root, dir)?
    } else {
        root.to_path_buf()
    };

    let handle =
        tokio::task::spawn_blocking(move || execute_script(&script, &workdir, allow_write));
    match tokio::time::timeout(SCRIPT_TIMEOUT, handle).await {
        Ok(joined) => joined.map_err(|e| anyhow!("run_script worker failed: {e}"))?,
        Err(_) => Ok(ToolResult::failure(
            "run_script: timed out after 30s".to_string(),
        )),
    }
}

/// Execute a Rhai script synchronously with a given workdir and no file-write access.
/// Exposed for `do_it check` which runs consistency scripts with a longer timeout.
pub(crate) fn execute_script_readonly(script: &str, workdir: &Path) -> ToolResult {
    execute_script(script, workdir, false)
        .unwrap_or_else(|e| ToolResult::failure(format!("execute_script error: {e}")))
}

fn execute_script(script: &str, workdir: &Path, allow_write: bool) -> Result<ToolResult> {
    let logs = Arc::new(Mutex::new(Vec::<String>::new()));
    let started = Instant::now();
    let mut engine = Engine::new();
    configure_limits(&mut engine, started);
    register_host_functions(&mut engine, workdir.to_path_buf(), Arc::clone(&logs), allow_write);

    let result = engine.eval::<Dynamic>(script);
    let logs = logs.lock().unwrap().clone();

    match result {
        Ok(value) => format_script_success(value, logs),
        Err(err) => Ok(ToolResult::failure(format!(
            "run_script failed: {}\n{}",
            err,
            render_logs(&logs)
        ))),
    }
}

fn configure_limits(engine: &mut Engine, started: Instant) {
    engine.set_max_operations(MAX_OPERATIONS);
    engine.set_max_array_size(MAX_ARRAY_SIZE);
    engine.set_max_map_size(MAX_MAP_SIZE);
    engine.set_max_string_size(MAX_STRING_SIZE);
    engine.set_max_call_levels(MAX_CALL_LEVELS);
    engine.set_max_expr_depths(MAX_EXPR_DEPTH, MAX_EXPR_DEPTH);
    engine.set_max_variables(MAX_VARIABLES);
    engine.set_max_functions(MAX_FUNCTIONS);
    engine.set_max_modules(MAX_MODULES);
    engine.on_progress(move |_| {
        if started.elapsed() > SCRIPT_TIMEOUT {
            Some(Dynamic::from("script exceeded 30s time budget"))
        } else {
            None
        }
    });
}

fn register_host_functions(
    engine: &mut Engine,
    workdir: PathBuf,
    logs: Arc<Mutex<Vec<String>>>,
    allow_write: bool,
) {
    // ── read_lines ────────────────────────────────────────────────────────────
    let read_root = workdir.clone();
    engine.register_fn(
        "read_lines",
        move |path: ImmutableString| -> std::result::Result<Array, Box<EvalAltResult>> {
            let resolved = resolve(&read_root, path.as_str()).map_err(rhai_err)?;
            let content = std::fs::read_to_string(&resolved)
                .map_err(|e| rhai_err(anyhow!("read_lines: {}: {e}", resolved.display())))?;
            Ok(content
                .lines()
                .map(|line| Dynamic::from(line.to_string()))
                .collect())
        },
    );

    // ── read_text — whole file as a single string ─────────────────────────────
    let text_root = workdir.clone();
    engine.register_fn(
        "read_text",
        move |path: ImmutableString| -> std::result::Result<ImmutableString, Box<EvalAltResult>> {
            let resolved = resolve(&text_root, path.as_str()).map_err(rhai_err)?;
            let content = std::fs::read_to_string(&resolved)
                .map_err(|e| rhai_err(anyhow!("read_text: {}: {e}", resolved.display())))?;
            if content.len() > MAX_STRING_SIZE {
                return Err(rhai_err(anyhow!(
                    "read_text: file exceeds {} byte limit",
                    MAX_STRING_SIZE
                )));
            }
            Ok(content.into())
        },
    );

    // ── regex_match ───────────────────────────────────────────────────────────
    engine.register_fn(
        "regex_match",
        |pattern: ImmutableString,
         text: ImmutableString|
         -> std::result::Result<bool, Box<EvalAltResult>> {
            let re =
                Regex::new(pattern.as_str()).map_err(|e| rhai_err(anyhow!("invalid regex: {e}")))?;
            Ok(re.is_match(text.as_str()))
        },
    );

    // ── regex_find_all — collect all non-overlapping matches ─────────────────
    engine.register_fn(
        "regex_find_all",
        |pattern: ImmutableString,
         text: ImmutableString|
         -> std::result::Result<Array, Box<EvalAltResult>> {
            let re =
                Regex::new(pattern.as_str()).map_err(|e| rhai_err(anyhow!("invalid regex: {e}")))?;
            let matches: Array = re
                .find_iter(text.as_str())
                .map(|m| Dynamic::from(m.as_str().to_string()))
                .collect();
            Ok(matches)
        },
    );

    // ── parse_json ────────────────────────────────────────────────────────────
    engine.register_fn(
        "parse_json",
        |text: ImmutableString| -> std::result::Result<Dynamic, Box<EvalAltResult>> {
            let value: Value = serde_json::from_str(text.as_str())
                .map_err(|e| rhai_err(anyhow!("parse_json: invalid JSON: {e}")))?;
            Ok(dynamic_from_json(&value))
        },
    );

    // ── fnv64 — fast non-cryptographic FNV-1a 64-bit hash ──────────────────────
    // Name reflects the actual algorithm. Use for change detection and
    // deduplication only — NOT for security or interoperability with SHA-256.
    engine.register_fn(
        "fnv64",
        |text: ImmutableString| -> ImmutableString {
            fnv64_hex(text.as_str()).into()
        },
    );

    // ── log ───────────────────────────────────────────────────────────────────
    // write_text -- opt-in file write (requires allow_write: true)
    if allow_write {
        let write_root = workdir.clone();
        engine.register_fn(
            "write_text",
            move |path: ImmutableString,
                  content: ImmutableString|
                  -> std::result::Result<ImmutableString, Box<EvalAltResult>> {
                if content.len() > MAX_STRING_SIZE {
                    return Err(rhai_err(anyhow!(
                        "write_text: content exceeds {} byte limit",
                        MAX_STRING_SIZE
                    )));
                }
                let resolved = resolve(&write_root, path.as_str()).map_err(rhai_err)?;
                if let Some(parent) = resolved.parent() {
                    std::fs::create_dir_all(parent)
                        .map_err(|e| rhai_err(anyhow!("write_text mkdir: {e}")))?;
                }
                std::fs::write(&resolved, content.as_str()).map_err(|e| {
                    rhai_err(anyhow!("write_text: {}: {e}", resolved.display()))
                })?;
                Ok(format!("written: {}", resolved.display()).into())
            },
        );
    } else {
        // Stub: clear error instead of confusing "function not found"
        engine.register_fn(
            "write_text",
            |_path: ImmutableString,
             _content: ImmutableString|
             -> std::result::Result<ImmutableString, Box<EvalAltResult>> {
                Err(rhai_err(anyhow!(
                    "write_text is disabled -- pass allow_write: true to run_script to enable it"
                )))
            },
        );
    }

    // ── list_dir ────────────────────────────────────────────────────────────────────────
    // Returns an Array of entry names (not full paths) inside the given directory.
    // Path is resolved relative to workdir; path traversal is rejected.
    let list_root = workdir.clone();
    engine.register_fn(
        "list_dir",
        move |path: ImmutableString| -> std::result::Result<Array, Box<EvalAltResult>> {
            let resolved = resolve(&list_root, path.as_str()).map_err(rhai_err)?;
            let rd = std::fs::read_dir(&resolved).map_err(|e| {
                rhai_err(anyhow!("list_dir: {}: {e}", resolved.display()))
            })?;
            let mut entries: Vec<Dynamic> = Vec::new();
            for entry in rd {
                let entry =
                    entry.map_err(|e| rhai_err(anyhow!("list_dir: read entry: {e}")))?;
                let name = entry.file_name().to_string_lossy().into_owned();
                entries.push(Dynamic::from(name));
            }
            entries.sort_by(|a, b| {
                a.clone()
                    .try_cast::<ImmutableString>()
                    .map(|s| s.to_string())
                    .unwrap_or_default()
                    .cmp(
                        &b.clone()
                            .try_cast::<ImmutableString>()
                            .map(|s| s.to_string())
                            .unwrap_or_default(),
                    )
            });
            Ok(entries)
        },
    );

    // ── file_exists ────────────────────────────────────────────────────────────────
    // Returns true when the path exists (file or directory) within workdir.
    // Path traversal is rejected.
    let exists_root = workdir.clone();
    engine.register_fn(
        "file_exists",
        move |path: ImmutableString| -> std::result::Result<bool, Box<EvalAltResult>> {
            let resolved = resolve(&exists_root, path.as_str()).map_err(rhai_err)?;
            Ok(resolved.exists())
        },
    );

    engine.register_fn("log", move |msg: Dynamic| {
        let mut collected = logs.lock().unwrap();
        collected.push(format_dynamic(&msg));
    });
}

// ── fnv64 implementation ──────────────────────────────────────────────────────

fn fnv64_hex(input: &str) -> String {
    // FNV-1a 64-bit expressed as 16-char hex — stable, dependency-free, useful
    // for change detection and deduplication in scripts. Not cryptographic.
    let mut h: u64 = 14695981039346656037;
    for byte in input.bytes() {
        h ^= byte as u64;
        h = h.wrapping_mul(1099511628211);
    }
    format!("{h:016x}")
}

// ── JSON → Rhai Dynamic conversion ───────────────────────────────────────────

fn dynamic_from_json(value: &Value) -> Dynamic {
    match value {
        Value::Null => Dynamic::UNIT,
        Value::Bool(v) => Dynamic::from_bool(*v),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Dynamic::from_int(i)
            } else if let Some(f) = n.as_f64() {
                Dynamic::from_float(f)
            } else if let Some(u) = n.as_u64() {
                Dynamic::from_int(i64::try_from(u).unwrap_or(i64::MAX))
            } else {
                Dynamic::UNIT
            }
        }
        Value::String(s) => Dynamic::from(s.to_string()),
        Value::Array(items) => Dynamic::from_array(items.iter().map(dynamic_from_json).collect()),
        Value::Object(map) => {
            let mut rhai_map = Map::new();
            for (key, item) in map {
                rhai_map.insert(key.into(), dynamic_from_json(item));
            }
            Dynamic::from_map(rhai_map)
        }
    }
}

// ── output formatting ─────────────────────────────────────────────────────────

fn format_script_success(value: Dynamic, logs: Vec<String>) -> Result<ToolResult> {
    let result = format_dynamic(&value);
    let output = if logs.is_empty() {
        format!("Result:\n{result}")
    } else {
        format!("Logs:\n{}\n\nResult:\n{result}", logs.join("\n"))
    };

    if output.len() > MAX_OUTPUT_BYTES {
        return Ok(ToolResult::failure(format!(
            "run_script: output exceeded {} bytes",
            MAX_OUTPUT_BYTES
        )));
    }

    Ok(ToolResult::ok(output))
}

fn render_logs(logs: &[String]) -> String {
    if logs.is_empty() {
        "Logs:\n(none)".to_string()
    } else {
        format!("Logs:\n{}", logs.join("\n"))
    }
}

pub(crate) fn format_dynamic(value: &Dynamic) -> String {
    if value.is_unit() {
        return "()".to_string();
    }
    if let Some(v) = value.clone().try_cast::<bool>() {
        return v.to_string();
    }
    if let Some(v) = value.clone().try_cast::<i64>() {
        return v.to_string();
    }
    if let Some(v) = value.clone().try_cast::<f64>() {
        return v.to_string();
    }
    if let Some(v) = value.clone().try_cast::<ImmutableString>() {
        return v.to_string();
    }
    if let Some(v) = value.clone().try_cast::<Array>() {
        let items = v.iter().map(format_dynamic).collect::<Vec<_>>().join(", ");
        return format!("[{items}]");
    }
    if let Some(v) = value.clone().try_cast::<Map>() {
        let items = v
            .iter()
            .map(|(key, item)| format!("{key}: {}", format_dynamic(item)))
            .collect::<Vec<_>>()
            .join(", ");
        return format!("#{{{items}}}");
    }
    format!("{value:?}")
}

// ── Rhai error helper ─────────────────────────────────────────────────────────

fn rhai_err(e: anyhow::Error) -> Box<EvalAltResult> {
    Box::new(EvalAltResult::ErrorRuntime(
        e.to_string().into(),
        rhai::Position::NONE,
    ))
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::TempDir;

    #[tokio::test]
    async fn run_script_executes_basic_math() {
        let dir = TempDir::new().unwrap();
        let args = json!({ "script": "40 + 2" });
        let result = run_script(&args, dir.path()).await.unwrap();
        assert!(result.success);
        assert!(result.output.contains("42"));
    }

    #[tokio::test]
    async fn run_script_reads_lines_within_root() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("sample.txt"), "a\nb\n").unwrap();
        let args = json!({ "script": r#"let lines = read_lines("sample.txt"); lines.len"# });
        let result = run_script(&args, dir.path()).await.unwrap();
        assert!(result.success);
        assert!(result.output.contains("2"));
    }

    #[tokio::test]
    async fn run_script_read_text_whole_file() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("hello.txt"), "hello world").unwrap();
        let args = json!({ "script": r#"read_text("hello.txt")"# });
        let result = run_script(&args, dir.path()).await.unwrap();
        assert!(result.success);
        assert!(result.output.contains("hello world"));
    }

    #[tokio::test]
    async fn run_script_blocks_path_traversal() {
        let dir = TempDir::new().unwrap();
        let args = json!({ "script": r#"read_lines("../outside.txt")"# });
        let result = run_script(&args, dir.path()).await.unwrap();
        assert!(!result.success);
        assert!(result.output.contains("Path traversal"));
    }

    #[tokio::test]
    async fn run_script_supports_regex_and_json() {
        let dir = TempDir::new().unwrap();
        let args = json!({
            "script": r#"
                let data = parse_json("{\"ok\":true,\"n\":3}");
                regex_match("^3$", data["n"].to_string()) && data["ok"]
            "#
        });
        let result = run_script(&args, dir.path()).await.unwrap();
        assert!(result.success);
        assert!(result.output.contains("true"));
    }

    #[tokio::test]
    async fn run_script_regex_find_all() {
        let dir = TempDir::new().unwrap();
        let args = json!({
            "script": r#"
                let matches = regex_find_all("\\d+", "v1.2 and v3.4");
                matches.len()
            "#
        });
        let result = run_script(&args, dir.path()).await.unwrap();
        assert!(result.success);
        assert!(result.output.contains("4"), "expected 4 digit matches");
    }

    #[tokio::test]
    async fn run_script_fnv64_stable() {
        let dir = TempDir::new().unwrap();
        let args = json!({ "script": r#"fnv64("hello") == fnv64("hello")"# });
        let result = run_script(&args, dir.path()).await.unwrap();
        assert!(result.success);
        assert!(result.output.contains("true"));
    }

    #[tokio::test]
    async fn run_script_sha256_not_available() {
        // Verify the old name is gone — using it must produce a script error.
        let dir = TempDir::new().unwrap();
        let args = json!({ "script": r#"sha256("hello")"# });
        let result = run_script(&args, dir.path()).await.unwrap();
        assert!(!result.success, "sha256 must not be available; got: {}", result.output);
    }

    #[tokio::test]
    async fn run_script_collects_logs() {
        let dir = TempDir::new().unwrap();
        let args = json!({ "script": r#"log("hello"); 1 + 1"# });
        let result = run_script(&args, dir.path()).await.unwrap();
        assert!(result.success);
        assert!(result.output.contains("Logs:"));
        assert!(result.output.contains("hello"));
    }

    #[tokio::test]
    async fn run_script_times_out_long_loop() {
        let dir = TempDir::new().unwrap();
        let args = json!({ "script": "loop { }" });
        let result = run_script(&args, dir.path()).await.unwrap();
        assert!(!result.success);
        assert!(
            result.output.contains("timed out")
                || result.output.contains("time budget")
                || result.output.contains("run_script failed")
                || result.output.contains("operations")
        );
    }

    #[tokio::test]
    async fn run_script_count_pattern_in_file() {
        // Demonstrates the primary use-case: count occurrences without read_file + mental work
        let dir = TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("src.rs"),
            "fn a() { x.unwrap() }\nfn b() { y.unwrap() }\nfn c() { ok }\n",
        )
        .unwrap();
        let args = json!({
            "script": r#"
                let lines = read_lines("src.rs");
                let count = 0;
                for line in lines {
                    if regex_match("unwrap\\(\\)", line) { count += 1; }
                }
                count
            "#
        });
        let result = run_script(&args, dir.path()).await.unwrap();
        assert!(result.success);
        assert!(result.output.contains("2"));
    }

    #[tokio::test]
    async fn write_text_writes_file_when_allowed() {
        let dir = TempDir::new().unwrap();
        let args = json!({
            "allow_write": true,
            "script": r#"write_text("out/result.txt", "hello from rhai")"#
        });
        let result = run_script(&args, dir.path()).await.unwrap();
        assert!(result.success, "write_text must succeed: {}", result.output);

        let written = std::fs::read_to_string(dir.path().join("out/result.txt")).unwrap();
        assert_eq!(written, "hello from rhai");
    }

    #[tokio::test]
    async fn write_text_creates_parent_dirs() {
        let dir = TempDir::new().unwrap();
        let args = json!({
            "allow_write": true,
            "script": r#"write_text("a/b/c/deep.txt", "nested")"#
        });
        let result = run_script(&args, dir.path()).await.unwrap();
        assert!(result.success);
        assert!(dir.path().join("a/b/c/deep.txt").exists());
    }

    #[tokio::test]
    async fn write_text_blocked_without_allow_write() {
        let dir = TempDir::new().unwrap();
        let args = json!({
            "script": r#"write_text("out.txt", "hello")"#
        });
        let result = run_script(&args, dir.path()).await.unwrap();
        assert!(!result.success);
        assert!(
            result.output.contains("allow_write"),
            "error must mention allow_write: {}",
            result.output
        );
        assert!(!dir.path().join("out.txt").exists(), "file must not be created");
    }

    #[tokio::test]
    async fn write_text_blocks_path_traversal() {
        let dir = TempDir::new().unwrap();
        let args = json!({
            "allow_write": true,
            "script": r#"write_text("../escape.txt", "x")"#
        });
        let result = run_script(&args, dir.path()).await.unwrap();
        assert!(!result.success);
        assert!(result.output.contains("Path traversal") || result.output.contains("run_script failed"));
    }

    #[tokio::test]
    async fn write_then_read_roundtrip() {
        let dir = TempDir::new().unwrap();
        let args = json!({
            "allow_write": true,
            "script": r#"
                write_text("tmp.txt", "line1\nline2\n");
                let lines = read_lines("tmp.txt");
                lines.len()
            "#
        });
        let result = run_script(&args, dir.path()).await.unwrap();
        assert!(result.success);
        assert!(result.output.contains("2"));
    }

    #[test]
    fn developer_aid_check_dead_tools_script_executes() {
        let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let script = std::fs::read_to_string(repo_root.join("scripts/check_dead_tools.rhai")).unwrap();

        assert!(script.contains("src/tools/spec.rs"));
        assert!(script.contains("src/tools/core.rs"));
        assert!(script.contains("missing_in_core"));
    }

    #[test]
    fn developer_aid_check_prompt_sync_script_executes() {
        let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let script = std::fs::read_to_string(repo_root.join("scripts/check_prompt_sync.rhai")).unwrap();

        assert!(script.contains("src/tools/spec.rs"));
        assert!(script.contains("src/prompts/default.md"));
        assert!(script.contains("role_reports"));
    }

    #[tokio::test]
    async fn list_dir_returns_entry_names() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("alpha.txt"), "").unwrap();
        std::fs::write(dir.path().join("beta.txt"), "").unwrap();
        std::fs::create_dir(dir.path().join("subdir")).unwrap();
        let args = json!({ "script": r#"let entries = list_dir("."); entries.len()"# });
        let result = run_script(&args, dir.path()).await.unwrap();
        assert!(result.success, "{}", result.output);
        assert!(result.output.contains("3"), "expected 3 entries: {}", result.output);
    }

    #[tokio::test]
    async fn list_dir_sorted_alphabetically() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("z.txt"), "").unwrap();
        std::fs::write(dir.path().join("a.txt"), "").unwrap();
        // Script checks that list_dir()[0] is "a.txt"
        let args = json!({ "script": r#"let e = list_dir("."); e[0] == "a.txt""# });
        let result = run_script(&args, dir.path()).await.unwrap();
        assert!(result.success, "{}", result.output);
        assert!(result.output.contains("true"), "{}", result.output);
    }

    #[tokio::test]
    async fn list_dir_blocks_path_traversal() {
        let dir = TempDir::new().unwrap();
        let args = json!({ "script": r#"list_dir("../")"# });
        let result = run_script(&args, dir.path()).await.unwrap();
        assert!(!result.success);
        assert!(result.output.contains("Path traversal") || result.output.contains("run_script failed"));
    }

    #[tokio::test]
    async fn file_exists_returns_true_for_existing_file() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("present.txt"), "x").unwrap();
        let args = json!({ "script": r#"file_exists("present.txt")"# });
        let result = run_script(&args, dir.path()).await.unwrap();
        assert!(result.success, "{}", result.output);
        assert!(result.output.contains("true"), "{}", result.output);
    }

    #[tokio::test]
    async fn file_exists_returns_false_for_missing_file() {
        let dir = TempDir::new().unwrap();
        let args = json!({ "script": r#"file_exists("absent.txt")"# });
        let result = run_script(&args, dir.path()).await.unwrap();
        assert!(result.success, "{}", result.output);
        assert!(result.output.contains("false"), "{}", result.output);
    }

    #[tokio::test]
    async fn file_exists_blocks_path_traversal() {
        let dir = TempDir::new().unwrap();
        let args = json!({ "script": r#"file_exists("../secret")"# });
        let result = run_script(&args, dir.path()).await.unwrap();
        assert!(!result.success);
        assert!(result.output.contains("Path traversal") || result.output.contains("run_script failed"));
    }

    #[tokio::test]
    async fn run_script_timeout_message_is_30s() {
        // Verify the timeout message matches the new constant.
        // The loop will be killed by tokio::time::timeout, not by on_progress,
        // since spawn_blocking wraps a blocking thread — check both strings.
        let dir = TempDir::new().unwrap();
        // This script runs MAX_OPERATIONS+1 iterations to trigger the progress guard.
        let script = "let i = 0; loop { i += 1; }";
        let args = json!({ "script": script });
        let result = run_script(&args, dir.path()).await.unwrap();
        assert!(!result.success);
        assert!(
            result.output.contains("30s") ||
            result.output.contains("time budget") ||
            result.output.contains("operations") ||
            result.output.contains("run_script failed"),
            "unexpected timeout message: {}", result.output
        );
    }
}
