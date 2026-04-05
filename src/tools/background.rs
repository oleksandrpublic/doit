use anyhow::{Result, anyhow};
use lazy_static::lazy_static;
use serde_json::Value;
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;

use crate::tools::core::resolve;
use crate::tools::tool_result::ToolResult;

const MAX_CAPTURE_LINES: usize = 500;

lazy_static! {
    static ref BACKGROUND_PROCESSES: Mutex<HashMap<u32, Arc<TrackedProcess>>> =
        Mutex::new(HashMap::new());
    static ref BACKGROUND_IDS: Mutex<HashMap<String, u32>> = Mutex::new(HashMap::new());
}

struct TrackedProcess {
    pid: u32,
    id: Option<String>,
    command: String,
    cwd: PathBuf,
    started_at: SystemTime,
    child: Mutex<Child>,
    stdout_lines: std::sync::Mutex<VecDeque<String>>,
    stderr_lines: std::sync::Mutex<VecDeque<String>>,
}

#[derive(Debug, Clone)]
struct SpawnRequest {
    id: Option<String>,
    program: String,
    args: Vec<String>,
    cwd: PathBuf,
    command_display: String,
}

pub async fn process_list(_args: &Value, _root: &Path) -> Result<ToolResult> {
    let processes = BACKGROUND_PROCESSES.lock().await;
    if processes.is_empty() {
        return Ok(ToolResult::ok("Background processes:\n(none)".to_string()));
    }

    let tracked: Vec<Arc<TrackedProcess>> = processes.values().cloned().collect();
    drop(processes);

    let mut lines = vec!["Background processes:".to_string()];
    for process in tracked {
        let (state, exit_code) = process.state().await?;
        let mut line = format!("- PID {} [{}]", process.pid, state);
        if let Some(id) = &process.id {
            line.push_str(&format!(" [{id}]"));
        }
        if let Some(code) = exit_code {
            line.push_str(&format!(" exit={code}"));
        }
        line.push_str(&format!(
            " cwd={} cmd={}",
            process.cwd.display(),
            process.command
        ));
        lines.push(line);
    }

    Ok(ToolResult::ok(lines.join("\n")))
}

pub async fn process_kill(args: &Value, _root: &Path) -> Result<ToolResult> {
    let process = match lookup_process(args).await? {
        Some(process) => process,
        None => {
            return Ok(ToolResult::failure(
                "Background process not found".to_string(),
            ));
        }
    };

    let mut child = process.child.lock().await;
    let already_exited = child.try_wait()?.is_some();
    if !already_exited {
        child.start_kill()?;
        let _ = child.wait().await;
    }
    drop(child);

    remove_process(process.pid, process.id.as_deref()).await;

    Ok(ToolResult::ok(format!(
        "Killed process {} ({})",
        process.pid, process.command
    )))
}

pub async fn run_background(args: &Value, root: &Path) -> Result<ToolResult> {
    let request = parse_spawn_request(args, root)?;

    if let Some(id) = &request.id {
        if BACKGROUND_IDS.lock().await.contains_key(id) {
            return Err(anyhow!("Background process id '{id}' is already in use"));
        }
    }

    let mut command = Command::new(&request.program);
    command.args(&request.args);
    command.current_dir(&request.cwd);
    command.stdout(std::process::Stdio::piped());
    command.stderr(std::process::Stdio::piped());

    let mut child = command.spawn().map_err(|e| {
        anyhow!(
            "Failed to spawn background process '{}': {e}",
            request.command_display
        )
    })?;

    let pid = child
        .id()
        .ok_or_else(|| anyhow!("Failed to determine PID for background process"))?;

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let process = Arc::new(TrackedProcess {
        pid,
        id: request.id.clone(),
        command: request.command_display.clone(),
        cwd: request.cwd.clone(),
        started_at: SystemTime::now(),
        child: Mutex::new(child),
        stdout_lines: std::sync::Mutex::new(VecDeque::new()),
        stderr_lines: std::sync::Mutex::new(VecDeque::new()),
    });

    if let Some(stdout) = stdout {
        spawn_capture_task(stdout, Arc::clone(&process), StreamKind::Stdout);
    }
    if let Some(stderr) = stderr {
        spawn_capture_task(stderr, Arc::clone(&process), StreamKind::Stderr);
    }

    {
        let mut processes = BACKGROUND_PROCESSES.lock().await;
        processes.insert(pid, Arc::clone(&process));
    }
    if let Some(id) = &request.id {
        BACKGROUND_IDS.lock().await.insert(id.clone(), pid);
    }

    Ok(ToolResult::ok(format!(
        "Background process started: PID {} ({})",
        pid, request.command_display
    )))
}

pub async fn process_status(args: &Value, _root: &Path) -> Result<ToolResult> {
    let process = match lookup_process(args).await? {
        Some(process) => process,
        None => return Ok(ToolResult::ok("No matching background process".to_string())),
    };

    let (state, exit_code) = process.state().await?;
    let stdout_excerpt = process.capture_excerpt(StreamKind::Stdout);
    let stderr_excerpt = process.capture_excerpt(StreamKind::Stderr);
    let started_secs = process
        .started_at
        .elapsed()
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let mut lines = vec![
        format!("PID {} [{}]", process.pid, state),
        format!("Command: {}", process.command),
        format!("Cwd: {}", process.cwd.display()),
        format!("Started: {}s ago", started_secs),
    ];
    if let Some(id) = &process.id {
        lines.push(format!("Id: {id}"));
    }
    if let Some(code) = exit_code {
        lines.push(format!("Exit code: {code}"));
    }
    if !stdout_excerpt.is_empty() {
        lines.push("stdout (recent):".to_string());
        lines.extend(stdout_excerpt);
    }
    if !stderr_excerpt.is_empty() {
        lines.push("stderr (recent):".to_string());
        lines.extend(stderr_excerpt);
    }

    Ok(ToolResult::ok(lines.join("\n")))
}

impl TrackedProcess {
    async fn state(&self) -> Result<(&'static str, Option<i32>)> {
        let mut child = self.child.lock().await;
        match child.try_wait()? {
            Some(status) => Ok(("exited", status.code())),
            None => Ok(("running", None)),
        }
    }

    fn capture_excerpt(&self, stream: StreamKind) -> Vec<String> {
        let store = match stream {
            StreamKind::Stdout => &self.stdout_lines,
            StreamKind::Stderr => &self.stderr_lines,
        };
        let lines = store.lock().unwrap();
        lines
            .iter()
            .rev()
            .take(10)
            .cloned()
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect()
    }
}

#[derive(Clone, Copy)]
enum StreamKind {
    Stdout,
    Stderr,
}

fn spawn_capture_task<R>(reader: R, process: Arc<TrackedProcess>, stream: StreamKind)
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut lines = BufReader::new(reader).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let target = match stream {
                StreamKind::Stdout => &process.stdout_lines,
                StreamKind::Stderr => &process.stderr_lines,
            };
            let mut store = target.lock().unwrap();
            if store.len() >= MAX_CAPTURE_LINES {
                store.pop_front();
            }
            store.push_back(line);
        }
    });
}

fn parse_spawn_request(args: &Value, root: &Path) -> Result<SpawnRequest> {
    let id = args
        .get("id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let cwd = if let Some(path) = args.get("cwd").and_then(|v| v.as_str()) {
        resolve(root, path)?
    } else {
        root.to_path_buf()
    };

    if let Some(cmd) = args.get("cmd").and_then(|v| v.as_str()) {
        return shell_request(cmd, id, cwd);
    }

    let program = args
        .get("program")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("Missing arg: cmd"))?
        .to_string();
    let arg_list = args
        .get("args")
        .and_then(|v| v.as_array())
        .map(|items| {
            items
                .iter()
                .map(|item| {
                    item.as_str()
                        .map(|s| s.to_string())
                        .ok_or_else(|| anyhow!("All args entries must be strings"))
                })
                .collect::<Result<Vec<_>>>()
        })
        .transpose()?
        .unwrap_or_default();

    let command_display = if arg_list.is_empty() {
        program.clone()
    } else {
        format!("{} {}", program, arg_list.join(" "))
    };

    Ok(SpawnRequest {
        id,
        program,
        args: arg_list,
        cwd,
        command_display,
    })
}

fn shell_request(cmd: &str, id: Option<String>, cwd: PathBuf) -> Result<SpawnRequest> {
    #[cfg(windows)]
    let (program, args) = ("cmd".to_string(), vec!["/C".to_string(), cmd.to_string()]);

    #[cfg(not(windows))]
    let (program, args) = ("sh".to_string(), vec!["-lc".to_string(), cmd.to_string()]);

    Ok(SpawnRequest {
        id,
        program,
        args,
        cwd,
        command_display: cmd.to_string(),
    })
}

async fn lookup_process(args: &Value) -> Result<Option<Arc<TrackedProcess>>> {
    let pid = if let Some(pid) = args.get("pid").and_then(|v| v.as_u64()) {
        Some(pid as u32)
    } else if let Some(id) = args.get("id").and_then(|v| v.as_str()) {
        BACKGROUND_IDS.lock().await.get(id).copied()
    } else {
        None
    };

    let Some(pid) = pid else {
        return Ok(None);
    };

    let process = BACKGROUND_PROCESSES.lock().await.get(&pid).cloned();
    Ok(process)
}

async fn remove_process(pid: u32, id: Option<&str>) {
    BACKGROUND_PROCESSES.lock().await.remove(&pid);
    if let Some(id) = id {
        BACKGROUND_IDS.lock().await.remove(id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_spawn_request_accepts_program_and_args() {
        let root = std::env::current_dir().unwrap();
        let args = json!({
            "id": "demo",
            "program": "cmd",
            "args": ["/C", "echo", "hello"],
            "cwd": "."
        });

        let req = parse_spawn_request(&args, &root).unwrap();
        assert_eq!(req.id.as_deref(), Some("demo"));
        assert_eq!(req.program, "cmd");
        assert_eq!(req.args, vec!["/C", "echo", "hello"]);
    }

    #[test]
    fn parse_spawn_request_accepts_cmd_string() {
        let root = std::env::current_dir().unwrap();
        let args = json!({ "cmd": "echo test" });
        let req = parse_spawn_request(&args, &root).unwrap();
        assert_eq!(req.command_display, "echo test");
        assert!(!req.program.is_empty());
    }

    #[test]
    fn parse_spawn_request_rejects_non_string_args() {
        let root = std::env::current_dir().unwrap();
        let args = json!({
            "program": "cmd",
            "args": [1]
        });
        let err = parse_spawn_request(&args, &root).unwrap_err().to_string();
        assert!(err.contains("All args entries must be strings"));
    }
}
