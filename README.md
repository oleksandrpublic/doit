# do_it

[![Crates.io](https://img.shields.io/crates/v/do_it.svg)](https://crates.io/crates/do_it)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

Autonomous coding agent for local repositories.

`do_it` reads, edits, tests, reviews, and documents code through focused agent roles, persistent project state, and a native Rust tool runtime. It runs on Windows and Linux, works with Ollama or cloud-compatible APIs, and keeps the technical surface explicit instead of hiding it behind shell glue.

Supports:
- `ollama`
- OpenAI-compatible backends
- Anthropic-compatible backends

---

## What It Does

- Multi-role agent workflow: `boss`, `developer`, `navigator`, `qa`, `reviewer`, `research`, `memory`
- Real sub-agent orchestration through `spawn_agent` / `spawn_agents`
- Native tools for filesystem edits, command execution, git, web, browser, memory, and code intelligence
- Persistent `.ai/` workspace with session reports, traces, prompts, knowledge, and runtime state
- Live TUI for interactive runs
- Optional capability groups: `browser`, `background`, `github`
- Dry-run environment validation via `do_it check`

---

## Quick Start

```bash
# 1. Install
cargo install do_it

# 2. Initialise a repo
cd /path/to/project
do_it init

# 3. Sanity-check the setup
do_it check

# 4. Run a task
do_it run --task "Find and fix the bug in src/parser.rs" --role developer
```

Boss orchestration for larger work:

```bash
do_it run --task "Plan and implement OAuth2 login end-to-end" --role boss --max-steps 80
```

For Ollama:

```bash
ollama pull qwen3.5:cloud
do_it init --backend ollama --model qwen3.5:cloud --yes
do_it check
```

---

## CLI

```text
do_it run     --task <text|file|image> [--repo <path>] [--config <path>] [--role <role>] [--system-prompt <text|file>] [--max-steps <n>]
do_it init    [--repo <path>] [--model <name>] [--backend <kind>] [--llm-url <url>] [--api-key <key>] [--yes]
do_it check   [--repo <path>] [--config <path>]
do_it status  [--repo <path>]
do_it config  [--config <path>]
do_it roles
```

`do_it check` currently validates:
- config loading
- static config validation
- runtime validation (`validate_runtime()`)
- `.ai/` workspace structure

For Ollama, runtime validation checks model reachability. For non-Ollama backends, model-list probing is currently skipped by policy.

---

## Roles

Each named role gets a smaller tool surface and a role-specific prompt. The `default` role is unrestricted.

| Role | Purpose | Typical tools |
|---|---|---|
| `boss` | Plan, delegate, coordinate, talk to the user | memory, tree, project_map, web_search, ask_human, notify, spawn_agent, spawn_agents |
| `developer` | Edit code and run narrow verification | read_file, open_file_region, write_file, str_replace, str_replace_multi, run_command, run_targeted_test, diff_repo, git tools |
| `navigator` | Explore code without editing | read_file, open_file_region, list_dir, find_files, search_in_files, outline, get_symbols, project_map, find_entrypoints, trace_call_path |
| `qa` | Run tests and verify changes | read_file, search_in_files, run_command, run_targeted_test, test_coverage, diff_repo, read_test_failure |
| `reviewer` | Static review, no execution | read_file, search_in_files, diff_repo, git_log, outline, get_signature, find_references |
| `research` | Web/doc lookup | web_search, fetch_url, memory, ask_human |
| `memory` | Manage agent memory entries | memory_read, memory_write, memory_delete |
| `default` | Unrestricted mode | all tools |

List the actual allowlists from the binary:

```bash
do_it roles
```

### Optional tool groups

```toml
tool_groups = ["browser", "background", "github"]
```

| Group | Adds |
|---|---|
| `browser` | `browser_action`, `browser_get_text`, `browser_navigate`, `screenshot` |
| `background` | `run_background`, `process_status`, `process_list`, `process_kill` |
| `github` | `github_api` |

---

## Important Capabilities

Notable tools and behaviors in the current codebase:

- `open_file_region` for focused reads around a line
- `str_replace_multi` for atomic multi-edit replacements
- `str_replace_fuzzy` and `apply_patch_preview` as experimental edit helpers
- `diff_repo` for repo diff inspection
- `find_entrypoints` and `trace_call_path` in workspace/code exploration
- `run_script` for sandboxed Rhai-based parsing and validation (`list_dir`, `file_exists`, `read_lines`, `read_text`, `regex_match`, `write_text`; 30s timeout)
- `checkpoint(note)` for mid-task progress recording
- `memory_delete` for explicit memory cleanup
- session traces with path-sensitivity summaries and redaction of persisted sensitive output

The authoritative tool registry is in [`src/tools/spec.rs`](D:/test/32/src/tools/spec.rs). Role prompts inject their tool catalogs from that registry at runtime, which keeps prompts, allowlists, and dispatch behavior aligned.

---

## `.ai/` Workspace

`do_it init` prepares the project workspace:

```text
.ai/
├── project.toml
├── prompts/
├── state/
├── logs/
├── knowledge/
├── tools/
└── screenshots/
```

Runtime also creates `.ai/memory/` on demand for memory tool keys.

Current notable artifacts:
- `.ai/state/current_plan.md`
- `.ai/state/last_session.md`
- `.ai/state/task_state.json`
- `.ai/state/checkpoints.md`
- `.ai/logs/session-NNN.md`
- `.ai/logs/session-NNN.trace.json`
- `.ai/knowledge/<key>.md`

`do_it status` summarizes the current workspace, recent session artifacts, wishlist, and knowledge keys.

---

## Configuration

Example `config.toml`:

```toml
llm_backend      = "ollama"
llm_url          = "http://localhost:11434"
# llm_api_key    = ""

model            = "qwen3.5:cloud"
temperature      = 0.0
max_tokens       = 4096
history_window   = 8
max_output_chars = 6000
max_depth        = 3
log_level        = "info"
log_format       = "text"

# Optional
# tool_groups = ["browser", "github"]

[models]
# coding    = "qwen3-coder-next:cloud"
# search    = "qwen3.5:9b"
# execution = "qwen3.5:9b"

# [browser]
# awp_url = "ws://127.0.0.1:9222"
```

Resolution order:
- `--config <path>`
- `<repo>/config.toml`
- `~/.do_it/config.toml`
- built-in defaults

---

## Current Notes

- `run_command` is explicit-args only, no shell pipeline syntax
- `run_targeted_test` and `format_changed_files_only` are intentionally Rust-first
- browser tools are optional and marked experimental
- `test_coverage` is Rust-oriented
- `do_it check` is the fastest way to catch config and workspace issues before a run

---

## More Detail

See [DOCS.md](D:/test/32/DOCS.md) for the fuller reference:
- CLI details
- tool families
- persistent state layout
- session lifecycle
- troubleshooting and limitations
