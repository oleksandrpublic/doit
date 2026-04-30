# do_it — Documentation

An autonomous coding agent powered by local or cloud LLMs.

Reads, writes, and fixes code in your repositories.

Runs on Windows and Linux with no shell dependency, no Python.

Supports **Ollama** (local), **OpenAI-compatible**, and **Anthropic-compatible** backends — including self-hosted services and providers such as MiniMax.

---

## Table of Contents

1. [Quick Start](#quick-start)
2. [Installation](#installation)
3. [Configuration](#configuration)
4. [CLI Reference](#cli-reference)
5. [Agent Roles](#agent-roles)
6. [Tools](#tools)
7. [Telegram — ask_human and notify](#telegram--ask_human-and-notify)
8. [Browser Tools](#browser-tools)
9. [Agent Self-Improvement](#agent-self-improvement)
10. [How the Agent Works](#how-the-agent-works)
11. [Model Selection and Routing](#model-selection-and-routing)
12. [Sub-agent Architecture](#sub-agent-architecture)
13. [Persistent Memory](#persistent-memory)
14. [Limitations](#limitations)
15. [Tips and Recommendations](#tips-and-recommendations)
16. [Troubleshooting](#troubleshooting)
17. [Project Structure](#project-structure)

---

## Quick Start

```bash
# 1. Install
cargo install do_it

# 2. Initialise project (interactive)
cd /path/to/project
do_it init

# 3. Validate configuration and workspace
do_it check

# 4. Run
do_it run --task "Find and fix the bug in src/parser.rs" --repo /path/to/project

# With a role (recommended for smaller models)
do_it run --task "Add input validation to handlers.rs" --role developer
```

With Ollama:
```bash
ollama pull qwen3.5:cloud
do_it init --backend ollama --model qwen3.5:cloud --yes
do_it check
```

With OpenAI or compatible:
```bash
do_it init --backend openai --llm-url https://api.openai.com --model gpt-4o
```

Windows:
```powershell
.\target\release\do_it.exe run `
  --task "Find and fix the bug in src/parser.rs" `
  --repo C:\Projects\my-project `
  --role developer
```

---

## Installation

### Requirements

- [Rust](https://rustup.rs/) 1.85+ (edition 2021)
- An LLM backend: [Ollama](https://ollama.com) locally, or any OpenAI / Anthropic compatible service

### Build from source

```bash
git clone https://github.com/oleksandrpublic/doit
cd doit
cargo build --release
```

Binary: `target/release/do_it` (Linux) / `target\release\do_it.exe` (Windows).

### Install from crates.io

```bash
cargo install do_it
```

### Backend setup

**Ollama (local):**
```bash
ollama pull qwen3.5:cloud   # recommended default
ollama list                 # verify installed models
curl http://localhost:11434/api/tags  # verify Ollama is running
```

**OpenAI or compatible service:** set `llm_url` and `llm_api_key` in `config.toml`, or run `do_it init` to generate a config interactively.

**Anthropic or compatible service:** same — set `llm_backend = "anthropic"`, `llm_url`, and `llm_api_key`.

---

## Configuration

Configuration is loaded from the first file found in this priority order:

1. `--config <path>` — explicit path passed on the command line
2. `./config.toml` — local project config (in the working directory)
3. `~/.do_it/config.toml` — global user config (created automatically on first run)
4. Built-in defaults

On first run, `~/.do_it/` is created with a default `config.toml` and supporting files.

```toml
# ── LLM backend ────────────────────────────────────────────────────────────────────────────────
# llm_backend: "ollama" | "openai" | "anthropic"
llm_backend      = "ollama"
llm_url          = "http://localhost:11434"
# llm_api_key    = ""          # or set LLM_API_KEY environment variable

# Default model — used when no role-specific override is set
model            = "qwen3.5:cloud"

# Sampling temperature: 0.0 = deterministic
temperature      = 0.0

# Max tokens per LLM response
max_tokens       = 4096

# Number of recent steps to include in full in context; older steps are collapsed to one line
history_window   = 8

# Max characters in tool output before truncation
max_output_chars = 6000

# Maximum sub-agent nesting depth
max_depth        = 3

# Logging configuration
log_level  = "info"    # "error", "warn", "info", "debug", "trace"
log_format = "text"    # "text", "json"

# Optional: Telegram for ask_human / notify
# telegram_token   = "1234567890:ABCdef..."
# telegram_chat_id = "123456789"

# Optional: per-action-type model overrides
[models]
# thinking  = "qwen3.5:cloud"
# coding    = "qwen3-coder-next:cloud"
# search    = "qwen3.5:9b"
# execution = "qwen3.5:9b"
# vision    = "qwen3.5:cloud"

# Optional: browser backend (AWP)
# [browser]
# awp_url        = "http://127.0.0.1:9222"
# screenshot_dir = ".ai/screenshots"
```

### Backend examples

**Local Ollama (default):**
```toml
llm_backend = "ollama"
llm_url     = "http://localhost:11434"
model       = "qwen3.5:cloud"
```

**Remote Ollama:**
```toml
llm_backend = "ollama"
llm_url     = "http://192.168.1.100:11434"
model       = "qwen3.5:35b"
```

**OpenAI:**
```toml
llm_backend = "openai"
llm_url     = "https://api.openai.com/v1"
llm_api_key = "sk-..."
model       = "gpt-4o"
```

**Anthropic:**
```toml
llm_backend = "anthropic"
llm_url     = "https://api.anthropic.com/v1"
llm_api_key = "sk-ant-..."
model       = "claude-sonnet-4-5-20251001"
```

**MiniMax (OpenAI-compatible):**
```toml
llm_backend = "openai"
llm_url     = "https://api.minimax.io/v1"
llm_api_key = "..."
model       = "abab6.5s-chat"
```

**Local proxy without key:**
```toml
llm_backend = "openai"
llm_url     = "http://localhost:20128/v1"
model       = "my-local-model"
# no llm_api_key needed
```

The `llm_api_key` field can also be supplied via the `LLM_API_KEY` environment variable.

### Defaults

| Field | Default |
|---|---|
| `llm_backend` | `ollama` |
| `llm_url` | `http://localhost:11434` |
| `llm_api_key` | _(none — set via env if needed)_ |
| `model` | `qwen3.5:cloud` |
| `temperature` | `0.0` |
| `max_tokens` | `4096` |
| `history_window` | `8` |
| `max_output_chars` | `6000` |
| `max_depth` | `3` |
| `log_level` | `info` |
| `log_format` | `text` |

### Optional tool groups

```toml
# Enable optional tool groups (browser, background, github)
# tool_groups = ["browser", "github"]
```

| Group | Tools | Roles |
|---|---|---|
| `browser` | `screenshot`, `browser_get_text`, `browser_action`, `browser_navigate` | boss, developer, qa, reviewer |
| `background` | `run_background`, `process_status`, `process_list`, `process_kill` | boss, developer |
| `github` | `github_api` | developer, qa |

```bash
do_it config              # print resolved config
do_it config --config custom.toml
```

---

## CLI Reference

```
do_it run     Run the agent on a task
do_it init    Initialise a project workspace
do_it check   Dry-run validation: config, runtime, workspace
do_it config  Print resolved config and exit
do_it roles   List all roles with their tool allowlists
do_it status  Show current project status
```

### `do_it check`

Validation command for CI and setup verification:

- config load source
- static config validation
- runtime validation (Ollama model reachability; non-Ollama backends log a warning, not an error)
- `.ai/` workspace structure
- optional tool group checks (`browser`, `github`, `background`)

Exits non-zero when any check fails.

### `do_it status`

Summarizes:

- session count and config source
- recent session markdown reports in `.ai/logs/`
- recent structured trace files in `.ai/logs/`
- compact path-sensitivity diagnostics from the latest structured trace
- `last_session.md`
- `current_plan.md`
- `~/.do_it/tool_wishlist.md`
- knowledge keys under `.ai/knowledge/`

### `run` arguments

```
--task, -t        Task text, path to a .md file, or path to an image
--repo, -r        Repository / working directory (default: .)
--config, -c      Path to config.toml (default: ./config.toml)
--role            Agent role: boss | research | developer | navigator | qa | reviewer | memory
--system-prompt   Override system prompt: inline text or path to a file
--max-steps       Maximum agent steps (default: 30)
```

### `init` arguments

```
--repo, -r        Repository / working directory (default: .)
--backend         LLM backend: ollama | openai | anthropic
--llm-url         LLM service URL
--model           Model name
--api-key         API key (alternative: LLM_API_KEY env var)
--yes, -y         Skip interactive prompts, use defaults
```

`do_it init` creates `.ai/` workspace, `config.toml`, `.ai/project.toml`, and `.gitignore` entries. If run without `--yes` it prompts for backend, URL, model, and API key interactively. Existing files are never overwritten.

### Examples

```bash
# Plain task
do_it run --task "Refactor the database module"

# With role (recommended)
do_it run --task "Add rate limiting" --role developer
do_it run --task "What does this project do?" --role navigator
do_it run --task "Find docs for tower-http middleware" --role research

# Orchestrate a complex task with sub-agents
do_it run --task "Add OAuth2 login" --role boss --max-steps 60

# Task from file
do_it run --task tasks/issue-42.md --role developer --repo ~/projects/my-app

# Screenshot with an error (vision mode)
do_it run --task error-screenshot.png --role developer

# Custom prompt for a specific task
do_it run --task "Review only, do not edit" --system-prompt prompts/reviewer.md

# More steps for complex tasks
do_it run --task "Refactor auth module" --role developer --max-steps 50

# Continue after interruption
do_it run --task "continue" --max-steps 50
```

`--task` and `--system-prompt`: if the value is a path to an existing file, its contents are read; if the extension is an image (`.png`, `.jpg`, `.webp`, etc.), vision mode is activated.

### Logging

```bash
RUST_LOG=info  do_it run --task "..."   # default
RUST_LOG=debug do_it run --task "..."   # includes raw LLM responses
RUST_LOG=error do_it run --task "..."   # errors only
```

---

## Agent Roles

Roles restrict the agent to a focused tool set and apply a role-specific system prompt. This is critical for smaller models — 6–8 tools instead of 20+ significantly improves output quality and reduces hallucinations.

```bash
do_it roles   # print all roles and their allowlists
```

Short aliases accepted by CLI/config parsing:

- `developer` / `dev`
- `navigator` / `nav`
- `reviewer` / `review`

### Role table

| Role | Purpose | Key capabilities |
|---|---|---|
| `default` | No restrictions | all tools |
| `boss` | Orchestration — delegates everything, never writes code | `memory`, `tree`, `project_map`, `web_search`, `ask_human`, `notify`, `spawn_agent/s`, `tool_request`, `capability_gap` |
| `research` | Information gathering | `web_search`, `fetch_url`, `memory`, `ask_human` |
| `developer` | Write and run code — uses navigator sub-agent for exploration | `read_file`, `open_file_region`, `write_file`, `str_replace`, `str_replace_multi`, `str_replace_fuzzy`, `apply_patch_preview`, `run_command`, `run_targeted_test`, `format_changed_files_only`, `run_script`, `diff_repo`, `git_*`, `memory`, `notify` |
| `navigator` | Explore codebase — read-only | `read_file`, `list_dir`, `find_files`, `search_in_files`, `tree`, `get_symbols`, `outline`, `find_references`, `project_map`, `find_entrypoints`, `trace_call_path`, `memory` |
| `qa` | Testing and verification | `read_file`, `search_in_files`, `run_command`, `run_script`, `diff_repo`, `read_test_failure`, `test_coverage`, `run_targeted_test`, `git_*`, `memory`, `notify` |
| `reviewer` | Static code review — no execution | `read_file`, `search_in_files`, `diff_repo`, `git_log`, `get_symbols`, `outline`, `get_signature`, `find_references`, `ask_human`, `memory` |
| `memory` | Managing `.ai/` state | `memory_read`, `memory_write`, `memory_delete` |

Optional groups extend named roles only when enabled in `config.toml`:

| Group | Tools | Roles |
|---|---|---|
| `browser` | `browser_action`, `browser_get_text`, `browser_navigate`, `screenshot` | boss, developer, qa, reviewer |
| `background` | `run_background`, `process_status`, `process_list`, `process_kill` | boss, developer |
| `github` | `github_api` | developer, qa |

### System prompt priority

```
--system-prompt  (highest)
  ↓
--role           → looks for .ai/prompts/<role>.md, falls back to built-in
  ↓
system_prompt in config.toml
  ↓
~/.do_it/system_prompt.md  (global override)
  ↓
built-in DEFAULT_SYSTEM_PROMPT  (lowest)
```

### Custom role prompts

Create `.ai/prompts/<role>.md` in the repository root to override the built-in prompt for that role:

```bash
mkdir -p .ai/prompts
cat > .ai/prompts/developer.md << 'EOF'
You are a Rust expert working on this codebase.
Always run `cargo clippy -- -D warnings` after edits.
Use thiserror for error types, never anyhow in library code.
All public functions require doc comments.
EOF
```

The file is picked up automatically — no restart needed.

---

## Tools

All tools are implemented in native Rust under `src/tools/`.

The canonical registry lives in `src/tools/spec.rs`. It defines canonical names, aliases, role availability, dispatch kind, and capability status. Role prompts inject their `## Available tools` section from that registry at runtime. Generated tool catalogs mark non-real tools:

- `[limited]` for stubbed tools
- `[experimental]` for tools with a narrower or unstable runtime contract

### Filesystem

| Tool | Arguments | Description |
|---|---|---|
| `read_file` | `path`, `start_line?`, `end_line?` | Read file with line numbers |
| `open_file_region` | `path`, `line`, `before?`, `after?` | Focused region around a line |
| `write_file` | `path`, `content` | Overwrite file, create directories if needed |
| `str_replace` | `path`, `old_str`, `new_str` | Replace a unique string in a file |
| `str_replace_multi` | `path`, `edits[]` | Atomic multi-edit replacement |
| `str_replace_fuzzy` | `path`, `old_str`, `new_str` | Whitespace-tolerant replacement `[experimental]` |
| `apply_patch_preview` | `path`, `content?` or `old_str` + `new_str` | Preview an edit as a unified diff, no write `[experimental]` |
| `list_dir` | `path?` | List directory contents (one level) |
| `find_files` | `pattern`, `dir?` | Find files by name: `*.rs`, `test*`, substring |
| `search_in_files` | `pattern`, `dir?`, `ext?` | Regex search across file contents |
| `tree` | `dir?`, `depth?`, `ignore?` | Recursive directory tree |

`apply_patch_preview` and `str_replace_fuzzy` are `[experimental]`. Graduation criteria for each are documented in `src/tools/spec.rs` next to their registry entries.

### Execution

| Tool | Arguments | Description |
|---|---|---|
| `run_command` | `program`, `args[]`, `cwd?`, `timeout_secs?` | Run a program with explicit args array (no shell) |
| `format_changed_files_only` | `dir?`, `check_only?`, `timeout_secs?` | Format only changed Rust files detected from git status `[experimental]` |
| `run_targeted_test` | `path?`, `test?`, `kind?`, `target?`, `dir?` | Run a narrow Rust test target `[experimental]` |
| `run_script` | `script`, `dir?` | Run a sandboxed Rhai script for lightweight parsing and summarization |
| `diff_repo` | `base?`, `staged?`, `stat?` | Git diff vs HEAD or any ref |
| `read_test_failure` | `path?`, `test?`, `index?` | Extract a failing test block from a log |
| `test_coverage` | `dir?`, `threshold?`, `timeout_secs?` | Run tests with coverage |

`run_script` sandbox API:

```rhai
read_lines("path")                 // Array of strings, one per line
read_text("path")                  // full file as one string
list_dir("path")                   // Array of entry names in a directory (sorted)
file_exists("path")                // bool — true when path exists
regex_match("pattern", text)       // bool
regex_find_all("pattern", text)    // Array of match strings
parse_json(text)                   // Map/Array/scalar
fnv64(text)                       // FNV-1a 64-bit hash as hex (non-cryptographic; change detection only)
log("message")                     // shown under "Logs:" in output
write_text("path", "content")      // only with allow_write: true
```

Sandbox limits: 30 second wall-clock, bounded operations/depth/string/array/map sizes, workspace-scoped filesystem, no network or process spawning.

### Background Processes

| Tool | Arguments | Description |
|---|---|---|
| `run_background` | `program`, `args[]`, `cwd?`, `id`, `cmd?` | Start a named background process |
| `process_status` | `id` or `pid` | Check if a background process is still running |
| `process_kill` | `id` or `pid` | Terminate a background process |
| `process_list` | | List all tracked background processes |

### Git

| Tool | Arguments | Description |
|---|---|---|
| `git_status` | `short?` | Working tree status and branch info |
| `git_commit` | `message`, `files?`, `allow_empty?` | Stage files and commit |
| `git_log` | `n?`, `path?`, `oneline?` | Commit history, optionally filtered by path |
| `git_stash` | `action`, `message?`, `index?` | Stash management: `push`, `pop`, `list`, `drop`, `show` |
| `git_pull` | `remote?`, `branch?` | Pull from remote repository |
| `git_push` | `remote?`, `branch?`, `force?` | Push to remote repository |

### Internet

| Tool | Arguments | Description |
|---|---|---|
| `web_search` | `query`, `max_results?` | Search via DuckDuckGo HTML endpoint (no API key required) |
| `fetch_url` | `url`, `selector?` | Fetch a web page and return readable text |
| `github_api` | `method`, `endpoint`, `body?`, `token?` | GitHub REST API |

`web_search` uses a proper DOM parser (`scraper` crate) for HTML extraction, handling minified pages and multi-line tags correctly. Rate limiting is applied via the shared global limiter.

`fetch_url` supports `http`, `https`, and `file://` URLs. SSRF protection is always applied. Optional `allowlist` and `blocklist` arrays restrict or block specific domains.

`github_api` requires a `GITHUB_TOKEN` environment variable (or `token` argument). Common endpoints:

```
GET  /repos/{owner}/{repo}/issues
GET  /repos/{owner}/{repo}/issues/{n}
POST /repos/{owner}/{repo}/issues/{n}/comments
GET  /repos/{owner}/{repo}/pulls
GET  /repos/{owner}/{repo}/contents/{path}
```

### Code Intelligence

Tree-sitter backend. Supports Rust, TypeScript/JavaScript, Python, C++, Kotlin. Detected by file extension.

| Tool | Arguments | Description |
|---|---|---|
| `get_symbols` | `path`, `kinds?` | List all symbols: fn, struct, class, impl, enum, trait, type, const |
| `outline` | `path` | Structural overview with signatures and line numbers |
| `get_signature` | `path`, `name`, `lines?` | Full signature + doc comment for a named symbol |
| `find_references` | `name`, `dir?`, `ext?` | All usages of a symbol across the codebase |
| `project_map` | `dir?`, `depth?` | Project layout summary |
| `find_entrypoints` | `dir?`, `depth?`, `limit?` | Locate app/CLI/web/test entrypoints |
| `trace_call_path` | `symbol`, `dir?`, `depth?` | Caller chain for a symbol |

### Memory (`.ai/` hierarchy)

| Tool | Arguments | Description |
|---|---|---|
| `memory_read` | `key` | Read a memory entry |
| `memory_write` | `key`, `content`, `append?` | Write or append to a memory entry |
| `memory_delete` | `key` | Delete a memory entry |
| `checkpoint` | `note` | Record mid-task progress without finishing |

**Logical key mapping (reserved keys):**

| Key | File |
|---|---|
| `plan` | `.ai/state/current_plan.md` |
| `last_session` | `.ai/state/last_session.md` |
| `external_messages` | `.ai/state/external_messages.md` |
| `user_profile` | `~/.do_it/user_profile.md` (global) |
| `boss_notes` | `~/.do_it/boss_notes.md` (global) |
| `tool_wishlist` | `~/.do_it/tool_wishlist.md` (global) |
| any other key | `.ai/memory/<key>.txt` (namespaced: `.ai/memory/<ns>/<key>.txt`) |

### Communication

| Tool | Arguments | Description |
|---|---|---|
| `ask_human` | `question` | Send a question via Telegram and wait up to 5 min for reply; falls back to console |
| `notify` | `message`, `silent?` | Send a one-way Telegram notification (non-blocking) |
| `finish` | `summary`, `success` | Signal task completion |

### Multi-agent

| Tool | Arguments | Description |
|---|---|---|
| `spawn_agent` | `role`, `task`, `memory_key?`, `max_steps?` | Delegate a subtask to a specialised sub-agent |
| `spawn_agents` | `agents[]`, `timeout_secs?` | Delegate multiple subtasks (see note below) |

### Browser

Browser tools are currently `[experimental]`. Enabled via `tool_groups = ["browser"]`.

| Tool | Arguments | Description |
|---|---|---|
| `screenshot` | `url?`, `path?` | Optionally navigate, then save a PNG screenshot |
| `browser_get_text` | `url?`, `selector?` | Read rendered text from the current page or URL |
| `browser_action` | `action`, `url`, `ref?`, `css?`, `value?`, `wait_ms?` | Interact: `click`, `type`, `hover`, `clear`, `select`, `scroll` (`url` required every call) `[experimental]` |
| `browser_navigate` | `url`, `wait_ms?` | Navigate and wait for page load. `file://` not supported — serve files via HTTP first. `[experimental]` |

### Self-improvement

| Tool | Arguments | Description |
|---|---|---|
| `tool_request` | `name`, `description`, `motivation`, `priority?` | Request a new tool — appends to `~/.do_it/tool_wishlist.md` |
| `capability_gap` | `context`, `impact` | Report a structural blind spot |

---

## Telegram — ask_human and notify

### Setup

1. Create a bot via [@BotFather](https://t.me/BotFather) → copy the token
2. Send `/start` to your bot
3. Find your `chat_id` via [@userinfobot](https://t.me/userinfobot)
4. Add to `config.toml`:

```toml
telegram_token   = "1234567890:ABCdef-ghijklmnop"
telegram_chat_id = "123456789"
```

### ask_human

Sends a question and waits up to 2 minutes for your reply. Your reply is returned to the agent as the tool result. Falls back to console stdin if Telegram is not configured or unreachable.

### notify

Sends a one-way message with no waiting. Used for progress updates and completion notices during long autonomous runs. Falls back to stdout if Telegram is not configured.

```json
{ "tool": "notify", "args": { "message": "OAuth implementation complete, running tests..." } }
```

### External messages (inbox)

To send instructions to the agent before its next run, write to `.ai/state/external_messages.md`. On startup, the agent reads this file, injects it into context, and clears it.

```bash
echo "## 2026-01-15 10:30
Please also update the README after the refactor." >> .ai/state/external_messages.md
```

The next run will show: `[inbox] 2 external message(s) received`.

---

## Browser Tools

Browser tools give the agent eyes — the ability to see rendered pages, interact with UI elements, and take screenshots. This is essential for JavaScript-heavy applications (React, Vue, Leptos) where `fetch_url` returns an empty shell.

### Setup

The agent uses the **AWP protocol** — a WebSocket-based JSON protocol for browser automation. The client connects to `ws://host:port/`, performs a session handshake (`awp.hello` → `session.create`), issues page commands, then closes the session cleanly (`session.close` + WebSocket Close frame).

```toml
# config.toml
[browser]
awp_url        = "http://127.0.0.1:9222"
# screenshot_dir = ".ai/screenshots"   # default: .ai/screenshots
```

Start the AWP server before running the agent:

```bash
# plasmate (recommended — lightweight, designed for AI agents)
plasmate serve --protocol awp --host 127.0.0.1 --port 9222
```

### How the agent uses browser tools

```
screenshot(url)          → PNG saved to .ai/screenshots/
browser_get_text(url)    → full page text after JS execution
browser_action(...)      → click/type/hover + implicit screenshot
browser_navigate(url)    → navigate + wait + screenshot
```

The Boss uses screenshots to verify UI work directly:

```
boss: spawn_agent("developer", "add login form to /login")
boss: screenshot("http://localhost:3080/login")
boss: browser_action("type", "#email", "test@example.com")
boss: browser_action("click", "#submit")
boss: screenshot("http://localhost:3080/dashboard")
```

### Vision model integration

`screenshot` returns the image as base64. Pass it to a vision-capable model:

```bash
do_it run --task screenshot.png --role developer
```

---

## Agent Self-Improvement

The Boss accumulates knowledge about missing capabilities across sessions. Two tools write to `~/.do_it/tool_wishlist.md`:

### tool_request

Called when the Boss encounters a missing capability for the **second time** in any session:

```json
{
  "tool": "tool_request",
  "args": {
    "name": "run_background",
    "description": "Run a process in the background and keep it alive",
    "motivation": "Need to start trunk serve and then navigate to localhost",
    "priority": "high"
  }
}
```

### capability_gap

Called when the Boss observes a structural blind spot with no specific solution:

```json
{
  "tool": "capability_gap",
  "args": {
    "context": "Developer wrote a Leptos component but I cannot see how it renders",
    "impact": "Visual bugs go undetected until manual review"
  }
}
```

```bash
cat ~/.do_it/tool_wishlist.md
```

---

## How the Agent Works

### Session initialisation

```
session_init():
  → increment .ai/state/session_counter.txt
  → read last_session.md   → inject into history as step 0
  → restore task_state.json from disk (if resume-worthy)
  → cache boss_notes.md and user_profile.md for prompt use
  → read external_messages.md → inject into context → clear
```

On first run in a new repository, `.ai/project.toml` is scaffolded automatically by detecting `Cargo.toml` / `package.json` / `pyproject.toml` / `go.mod`:

```toml
# .ai/project.toml (auto-generated, edit as needed)
[project]
name     = "my-project"
language = "rust"

[commands]
test  = "cargo test"
build = "cargo build --release"
lint  = "cargo clippy -- -D warnings"

[github]
repo = "owner/my-project"

[agent]
notes = """
- Always run clippy before committing
"""
```

### Main loop

```
if --task is an image:
  → vision model describes it → description becomes effective_task

for each step 1..max_steps:
  0. build prompt from:
     - task
     - working memory (goal, attempted_actions, artifacts_found, blocked_on, next_best_action)
     - recent history
     - status-aware strategy notes for weak/repeated tools
     - project context
  1. thinking model → JSON { thought, tool, args }
  2. canonicalize tool name through the registry
  3. check role tool allowlist (if role != Default)
  4. if specialist model differs → re-call with specialist
  5. execute tool
  6. update working memory from the tool result
  7. record in history
  8. loop detection / anti-loop policy
  9. if tool == "finish" → done
```

### Loop detection and anti-loop policy

- repeated identical `[limited]` or `[experimental]` tool calls trip loop handling sooner than normal tools
- prompts include `Strategy Notes` when a weak tool already returned the same result with the same args
- prompts include `Working Memory` to help the model remember blockers and next-best actions

### Resume behavior

Top-level sessions persist structured working memory in `.ai/state/task_state.json`.

- `continue` reuses the saved top-level goal when available
- the first prompt after restore includes resume guidance from the saved task state
- persisted task state is restored only for the top-level agent
- successful top-level completion clears the persisted snapshot
- failed or interrupted runs keep the snapshot for resume

Sub-agents intentionally do not restore persisted task state. They start with fresh in-memory working memory scoped to the current delegated task.

**Session recovery hierarchy:** resume data is pulled from multiple sources in priority order:
1. `task_state.json` — structured working memory (goal, actions, artifacts, blockers)
2. `.ai/state/last_session.md` — narrative note injected as step 0
3. stale plan file `.ai/state/current_plan.md` — used only when task_state is absent or empty

These sources do not conflict: `memory_read("plan")` maps to `.ai/state/current_plan.md`, and `memory_read("last_session")` maps to `.ai/state/last_session.md`, which matches where the lifecycle writes them.

### Context window

- Last `history_window` steps in full
- Older steps collapsed to one line: `step N ✓ [tool] — thought | first line of output`

### LLM response parsing

Finds the first `{` and last `}` — handles models that add prose before or after JSON. Strips ` ```json ` fences automatically.

### Tool error handling

A tool error does not stop the agent — it is recorded in history as a failed step. The agent can recover and try a different approach on the next step.

---

## Model Selection and Routing

### Recommended models (qwen3.5 family, for Ollama)

| Model | Size | Use case |
|---|---|---|
| `qwen3.5:cloud` | - | Default — good balance |
| `qwen3.5:35b` | 24 GB | Complex multi-step tasks |
| `qwen3.5:27b` | 17 GB | Complex multi-step tasks |
| `qwen3.5:9b` | 6.6 GB | Roles with ≤8 tools (navigator, research) |
| `qwen3-coder:30b` | 19 GB | Complex multi-step coding |
| `qwen3-coder-next:cloud` | - | Developer role, best code quality |

### Per-action-type model routing

```toml
model = "qwen3.5:cloud"   # fallback for all action types

[models]
coding    = "qwen3-coder-next:cloud"   # used for write_file, str_replace
search    = "qwen3.5:9b"               # used for read_file, find_files, etc.
execution = "qwen3.5:9b"               # used for run_command
vision    = "qwen3.5:cloud"            # used for image tasks
```

### LLM backends

| Backend | `llm_backend` value | Auth |
|---|---|---|
| Ollama | `ollama` | none (local) |
| OpenAI / compatible | `openai` | `llm_api_key` or `LLM_API_KEY` env |
| Anthropic / compatible | `anthropic` | `llm_api_key` or `LLM_API_KEY` env |

Any service implementing the OpenAI `/v1/chat/completions` or Anthropic `/v1/messages` API works. The `llm_url` field sets the base URL; the protocol suffix is added automatically.

---

## Sub-agent Architecture

`spawn_agent` lets the `boss` role delegate subtasks to specialised sub-agents. Each sub-agent runs in-process with its own history, role prompt, fresh in-memory working memory, and tool allowlist. Communication goes through the shared `.ai/knowledge/` memory.

Design rule:
- top-level agent: may restore persisted `TaskState`
- sub-agent: never restores persisted `TaskState`; always starts clean

### Usage

```json
{
  "tool": "spawn_agent",
  "args": {
    "role": "research",
    "task": "Find the best OAuth2 crates for Axum in 2026",
    "memory_key": "knowledge/oauth_research",
    "max_steps": 15
  }
}
```

### Arguments

| Argument | Required | Description |
|---|---|---|
| `role` | yes | Sub-agent role: `research`, `developer`, `navigator`, `qa`, `reviewer`, `memory` |
| `task` | yes | Task description |
| `memory_key` | no | Where to write results (default: `knowledge/agent_result`) |
| `max_steps` | no | Step limit (default: parent's `max_steps / 2`, min 5) |

Boss cannot spawn another boss (recursion guard). Maximum nesting depth is controlled by `max_depth` in config (default: 3).

### Full orchestration example

```bash
do_it run --task "Add OAuth2 login to the API" --role boss --max-steps 80
```

```
boss: reads last_session, plan, decisions, user_profile → writes task breakdown to plan
  │
  ├─ spawn_agent(role="research", task="find best OAuth crates",
  │              memory_key="knowledge/oauth_research")
  │
  ├─ spawn_agent(role="navigator", task="map existing auth structure",
  │              memory_key="knowledge/auth_structure")
  │
  ├─ spawn_agent(role="developer", task="implement OAuth2 per the plan",
  │              memory_key="knowledge/impl_notes")
  │
  ├─ spawn_agent(role="reviewer", task="review the implementation",
  │              memory_key="knowledge/review_report")
  │
  ├─ spawn_agent(role="qa", task="verify tests and coverage",
  │              memory_key="knowledge/qa_report")
  │
  └─ boss: reads reports → notify("OAuth2 complete") → finish
```

---

## Persistent Memory

The agent maintains persistent state in `.ai/` at the repository root. This directory is gitignored by default.

```
.ai/
├── project.toml           ← project config (auto-scaffolded, edit freely)
├── prompts/               ← custom role prompts (override built-ins per project)
│   ├── boss.md
│   ├── developer.md
│   └── qa.md
├── state/
│   ├── current_plan.md        ← boss writes the task plan here
│   ├── last_session.md        ← read on startup, written at end of session
│   ├── session_counter.txt
│   ├── task_state.json        ← structured working memory, survives interruption
│   ├── external_messages.md  ← external inbox, read and cleared on startup
│   ├── checkpoints.md         ← mid-task progress notes from checkpoint tool
│   └── session_decisions.md   ← per-step decision annotations from LLM actions
├── logs/
│   ├── history.md
│   ├── session-NNN.md         ← per-session markdown report
│   └── session-NNN.trace.json ← structured session trace
└── knowledge/                 ← agent-written project knowledge
    ├── lessons_learned.md     ← QA appends patterns after each session
    ├── decisions.md           ← Boss/Developer log architectural decisions
    └── qa_report.md           ← latest test run
```

### What each file is for

**`last_session.md`** — the agent writes a note to its future self at the end of every session: what was done, what is pending, any important context. Read automatically on next startup.

**`task_state.json`** — structured working memory persisted across interruptions. Restored at the start of the next session when it contains a resume-worthy goal or action history. Cleared on successful completion, kept on error or no-progress stop.

**`session-NNN.md`** — per-session markdown report. Includes task, summary, tool usage, and a `Path sensitivity` section when the session touched config, prompts, or other tagged path categories.

**`session-NNN.trace.json`** — structured session trace with start/turn/finish events, per-tool call counts, aggregated `path_sensitivity_stats`, and per-turn sensitivity hints.

**`session_decisions.md`** — the LLM can include a `decision` field in its JSON actions. These are automatically appended here per step — zero cost, no extra tool call needed.

**Redaction** — before any text is written to `session-NNN.md`, `session-NNN.trace.json`, or `last_session.md`, the task description and final summary pass through `src/redaction.rs`. The filter replaces lines containing known sensitive token patterns with `[redacted]`. The same filter is applied to write-oriented tool outputs before they are returned to the agent.

Covered patterns: PEM key headers, common API key prefixes (`sk-`, `ghp_`, `ghs_`, `glpat-`, `xoxb-`, `xoxp-`), HTTP auth headers, and common env-var-style assignments (`password=`, `secret=`, `api_key=`, `access_token=`, etc.).

**`external_messages.md`** — your inbox for the agent. Write anything here before a run; the agent sees it on startup and the file is cleared.

**`project.toml`** — permanent project context injected every session.

**`lessons_learned.md`** — QA appends project-specific anti-patterns after each session.

**`decisions.md`** — Boss and Developer log significant architectural decisions.

### Global memory — `~/.do_it/`

| File | Key | Purpose |
|---|---|---|
| `user_profile.md` | `user_profile` | Your preferences: language, tech stack, workflow style. Edit once, applies to all projects. |
| `boss_notes.md` | `boss_notes` | Cross-project insights the Boss accumulates. |

Both files are only injected into context when they contain actual content (not just `#` comment lines).

---

## Limitations

**Context window** — for long sessions reduce `history_window` to 4–5 and `max_output_chars` to 3000.

**`find_files`** — simple patterns only: `*.rs`, `test*`, substring. No `**`, `{a,b}`, or `?`.

**`run_command`** — no `|`, `&&`, `>`. Each command is a separate call with an explicit args array. Policy hardening: `program` must be a bare executable name from `PATH`; risky env overrides (`PATH`, `PATHEXT`, `LD_PRELOAD`, `CARGO_HOME`, etc.) are rejected; arg count/length and env var count/length are capped.

**`run_targeted_test`** — currently optimized for Rust repositories. Node/Python fallback branches are intentionally deferred.

**`format_changed_files_only`** — currently optimized for Rust repositories and relies on `git` + `rustfmt` being available.

**`apply_patch_preview`** — preview-only helper. Follow with `str_replace` or `write_file` for the real change.

**`str_replace_fuzzy`** — experimental whitespace-tolerant variant of `str_replace`. Graduation criteria documented in `src/tools/spec.rs`.

**`run_script`** — intentionally narrow. For small parsing/inspection tasks, not general-purpose program execution.

**`fetch_url`** — public URLs only, no authentication, no JavaScript rendering.

**`web_search`** — DuckDuckGo HTML endpoint (DOM-parsed via `scraper` crate), no API key required. Rate limiting possible under heavy use.

**Code intelligence** — tree-sitter backend covers ~95% of real-world cases. Does not handle macros, conditional compilation, or dynamically generated code.

**`test_coverage`** — implemented for Rust projects. Prefers `cargo llvm-cov`, falls back to `cargo tarpaulin`, then `cargo test` (no coverage numbers).

**Vision** — image input is implemented. GGUF files in Ollama may not include the mmproj component. Use llama.cpp directly or verify your model has vision support.

**`spawn_agents`** — executes sub-agents.

**`validate_runtime()`** — checks Ollama model reachability. Non-Ollama backends log a warning instead of probing models (no standard list endpoint).

---

## Tips and Recommendations

### Choose the right role

```bash
do_it run --task "What does this project do?"             --role navigator
do_it run --task "Find examples of using axum extractors" --role research
do_it run --task "Add validation to src/handlers.rs"      --role developer
do_it run --task "Do all tests pass?"                     --role qa
do_it run --task "Plan the auth module refactor"          --role boss
```

### Before running

```bash
git status              # start from a clean working tree
do_it check             # validate config and workspace
do_it config            # verify resolved config — check llm_url, llm_backend, model
```

### Write a good task description

```bash
# Vague — bad
do_it run --task "Improve the code"

# Specific — good
do_it run --task "In src/lexer.rs the tokenize function returns an empty Vec \
  for input containing only spaces. Fix it and add a regression test." --role developer

# From a file with full context
do_it run --task tasks/issue-42.md --role developer
```

### Project-specific role prompts

```bash
mkdir -p .ai/prompts
cat > .ai/prompts/developer.md << 'EOF'
You are a Rust expert on this codebase.
Always run `cargo clippy -- -D warnings` after edits.
Always run `cargo test` after edits.
Use thiserror for error types in library code.
All public items require doc comments.
EOF
```

### Using GitHub API

```bash
export GITHUB_TOKEN=ghp_xxxxxxxxxxxx
do_it run --task "Find all open bugs and fix the highest priority one" --role developer
```

---

## Troubleshooting

**`do_it check failed`** — run `do_it check` to see which check failed. Typical causes: invalid `config.toml`, unreachable Ollama URL, missing `.ai/` workspace.

**`Tool 'X' is not allowed for role 'Y'`** — the model tried to use a tool outside the role's allowlist. Either switch to `--role default` or add the tool to `.ai/prompts/<role>.md`.

**`Cannot reach LLM service`** — check that `llm_url` is correct and the service is running:
```bash
# Ollama
ollama serve
curl http://localhost:11434/api/tags

# OpenAI-compatible
curl http://localhost:20128/v1/models
```

**`Model 'X' not found`** (Ollama only)
```bash
ollama pull qwen3.5:cloud
ollama list
```

**`HTTP 401 Unauthorized`** — API key missing or wrong. Set `llm_api_key` in `config.toml` or the `LLM_API_KEY` environment variable.

**`HTTP 404` on chat endpoint** — `llm_url` may include an extra path. Use the base URL only; the protocol suffix is appended automatically (`/v1/chat/completions` for OpenAI, `/api/chat` for Ollama, `/v1/messages` for Anthropic).

**`LLM response has no JSON`** — the model responded outside JSON format. Try a larger model, set `temperature = 0.0`, or use a role to reduce the tool count.

**`str_replace: old_str found N times`** — provide more context to make `old_str` unique.

**`ask_human via Telegram: no reply received within 5 minutes`** — verify the bot token and chat_id, and that you have sent `/start` to the bot.

**`fetch_url: HTTP 403` or empty result** — the site blocks bots or requires JavaScript. Use direct API endpoints: `raw.githubusercontent.com` instead of `github.com`.

**`github_api: no token found`** — set `GITHUB_TOKEN` environment variable or pass `"token"` in args.

**`test_coverage: coverage backend not found`** — install `cargo-llvm-cov` or `cargo-tarpaulin`.

**`run_script` failed or timed out** — reduce the script size and keep it to parsing/transform tasks.

**`run_command` rejects a program or env override** — use a bare executable name from `PATH`, keep `timeout_secs` modest, and avoid overriding blocked environment keys.

**`apply_patch_preview` says `old_str` is missing or not unique** — mirrors `str_replace` rules on purpose.

**Agent is looping** — the runtime injects `Working Memory` and `Strategy Notes` to reduce repeated weak-tool calls, but loops are still possible. Reduce `history_window`, restart with a more specific task, or use a larger model.

**Sub-agent stuck** — limited by `max_steps`. Pass a smaller `max_steps` explicitly, or break the task into smaller pieces.

**Browser tool errors** — verify `[browser]` config and that the AWP server is running and accepting WebSocket connections at `awp_url`. Browser tools are experimental; connection and protocol errors are surfaced directly.

---

## Project Structure

```
do_it/
├── Cargo.toml           name="do_it", edition="2021", version="0.3.3"
├── config.toml          runtime configuration (models, Telegram, [browser])
├── README.md            project overview
├── DOCS.md              this file
├── LICENSE              MIT
├── .gitignore
├── .ai/                 agent memory (created automatically, gitignored)
│   ├── project.toml     project config (auto-scaffolded)
│   ├── prompts/         custom role prompts
│   ├── state/           plan, last_session, session_counter, task_state, external_messages
│   ├── logs/            history.md, per-session reports and traces
│   ├── screenshots/     browser tool output (PNG files)
│   └── knowledge/       lessons_learned, decisions, qa_report, sub-agent results
└── src/
    ├── main.rs              bin wrapper
    ├── start.rs             CLI: run | init | check | config | roles | status
    ├── start/
    │   ├── init.rs
    │   ├── check.rs         do_it check
    │   ├── run_support.rs
    │   ├── shared.rs
    │   ├── status.rs        do_it status
    │   └── tests.rs
    ├── lib.rs               library root for integration tests
    ├── agent/
    │   ├── mod.rs
    │   ├── core.rs          SweAgent struct and all field accessors
    │   ├── tools.rs         parse_action, LLM action helpers
    │   ├── session/         session_init/finish, task_state persistence, resume logic
    │   ├── prompt.rs        build_prompt, strategy_notes, loop detection
    │   ├── display.rs       console/TUI output helpers
    │   ├── spawn.rs         spawn_agent, spawn_agents
    │   └── loops/
    │       ├── mod.rs         run(), run_capture(), step()
    │       └── tests.rs
    ├── config_struct.rs     AgentConfig, BrowserConfig, Role enum, model router, built-in prompts
    ├── config_loader.rs     config loading, global config bootstrap, prompt overrides
    ├── config_validation.rs runtime and static config validation
    ├── history.rs           sliding window context manager
    ├── task_state.rs        structured working memory
    ├── loop_policy.rs       loop/stall detection thresholds
    ├── redaction.rs         central redaction filter
    ├── shell.rs             LLM client: Ollama / OpenAI / Anthropic backends
    ├── tui.rs               Ratatui TUI: three-panel live view, prompt widget
    ├── validation.rs        path traversal protection
    └── tools/
        ├── core.rs          central dispatch (dispatch_with_depth)
        ├── spec.rs          tool registry: names, aliases, roles, status, prompt metadata
        ├── file_ops.rs      read_file, write_file, str_replace, apply_patch_preview, ...
        ├── commands.rs      run_command, run_targeted_test, format_changed_files_only
        ├── web.rs           web_search (scraper), fetch_url, github_api
        ├── human.rs         ask_human (TUI + Telegram), notify
        ├── memory.rs        memory_read, memory_write, memory_delete
        ├── code_analysis.rs get_symbols, outline, get_signature, find_references
        ├── git.rs           git_status, git_commit, git_log, git_stash, git_pull, git_push
        ├── browser.rs       screenshot, browser_get_text, browser_action, browser_navigate (AWP)
        ├── workspace.rs     tree, project_map, find_entrypoints, trace_call_path, diff_repo
        ├── background.rs    run_background, process_status, process_list, process_kill
        ├── scripting.rs     run_script (Rhai sandbox)
        ├── cleanup.rs       log rotation, stale background pid cleanup
        └── test_coverage.rs test_coverage with cargo llvm-cov / tarpaulin fallback
```

Global config at `~/.do_it/` (created on first run):

```
~/.do_it/
├── config.toml          global defaults (overridden by local config.toml)
├── system_prompt.md     global default prompt override
├── user_profile.md      your preferences — Boss reads on every session start
├── boss_notes.md        cross-project insights accumulated by Boss
└── tool_wishlist.md     agent-requested capabilities — review to prioritise dev work
```

### Tool registry and prompt sync

The canonical registry is `src/tools/spec.rs`. It defines canonical names, aliases, role availability, dispatch kind, and capability status. Runtime dispatch lives in `src/tools/core.rs`. Role prompt catalogs are injected from the registry at runtime — they do not drift independently.

### Dependencies

| Crate | Purpose |
|---|---|
| `tokio` | async runtime |
| `reqwest` | HTTP — Ollama, fetch_url, web_search, Telegram API, GitHub API |
| `serde` / `serde_json` | JSON serialization |
| `toml` | config.toml / project.toml parsing |
| `clap` | CLI argument parsing |
| `walkdir` | recursive filesystem traversal |
| `regex` | search_in_files, find_references, parsers |
| `base64` | image encoding for vision; GitHub file contents decoding |
| `anyhow` | error handling |
| `tracing` / `tracing-subscriber` | structured logging |
| `scraper` | DOM parsing for web_search HTML extraction |
| `tree-sitter` | code intelligence backend |
| `rhai` | run_script sandboxed scripting |
| `ratatui` / `crossterm` | terminal UI |
| `plasmate` / AWP | browser automation via AWP WebSocket protocol (session lifecycle: hello → create → commands → close) |
